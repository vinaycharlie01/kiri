//! The adapter against a real Kiri daemon. Opt-in, so ordinary `cargo test`
//! needs no engine:
//!
//!   KAI_TEST_KIRI_SOCKET=$HOME/.kiri/run/docker.sock cargo test -p kai-adapter-kiri --test real_engine
//!
//! The lifecycle test also needs an image already present in the engine, and
//! a host that can run containers (on Linux: cgroup v2):
//!
//!   KAI_TEST_KIRI_IMAGE=alpine:3.21 …
//!
//! Every test cleans up what it creates. Without the variables each test
//! returns early and passes vacuously; the README of this crate says so.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::ops::ControlFlow;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use kai_adapter_kiri::KiriEngine;
use kai_application::{AppError, ContainerActionRequest, ContainerService};
use kai_domain::{
    Clock, ContainerAction, ContainerId, ContainerRuntime, ContainerState, DomainEvent,
    EngineEvent, EngineEventSource, EventPublisher, RuntimeError,
};

fn socket() -> Option<PathBuf> {
    std::env::var_os("KAI_TEST_KIRI_SOCKET").map(PathBuf::from)
}

/// A raw API call, for test setup the ports deliberately do not offer.
fn raw(sock: &PathBuf, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut s = UnixStream::connect(sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\nHost: kiri\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut rest = String::new();
    let _ = r.read_to_string(&mut rest);
    (status, rest)
}

#[test]
fn reports_the_engine() {
    let Some(sock) = socket() else { return };
    let info = KiriEngine::at(sock).engine_info().unwrap();
    assert!(!info.api_version.is_empty(), "{info:?}");
}

#[test]
fn lists_containers() {
    let Some(sock) = socket() else { return };
    KiriEngine::at(sock).list_containers(true).unwrap();
}

#[test]
fn an_unknown_container_is_none_and_acting_on_it_is_not_found() {
    let Some(sock) = socket() else { return };
    let e = KiriEngine::at(sock);
    let ghost = ContainerId::new("kai-test-no-such-container").unwrap();
    assert_eq!(e.container(&ghost).unwrap(), None);
    assert!(matches!(
        e.perform(&ghost, ContainerAction::Start),
        Err(RuntimeError::NotFound(_))
    ));
}

#[test]
fn follows_the_engines_event_stream() {
    let Some(sock) = socket() else { return };
    let name = format!("kai-test-vol-{}", std::process::id());
    let (s2, n2) = (sock.clone(), name.clone());
    // Make something happen once the stream is open.
    let poke = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        let (status, body) = raw(
            &s2,
            "POST",
            "/volumes/create",
            &format!(r#"{{"Name":"{n2}"}}"#),
        );
        assert_eq!(status, 201, "{body}");
    });
    let mut seen = None;
    KiriEngine::at(&sock)
        .follow(&mut |e| match e {
            EngineEvent::Other { kind, action } if kind == "volume" => {
                seen = Some(action);
                ControlFlow::Break(())
            }
            _ => ControlFlow::Continue(()),
        })
        .unwrap();
    poke.join().unwrap();
    raw(&sock, "DELETE", &format!("/volumes/{name}"), "");
    assert_eq!(seen.as_deref(), Some("create"));
}

struct Collect(std::sync::Mutex<Vec<DomainEvent>>);
impl EventPublisher for Collect {
    fn publish(&self, e: DomainEvent) {
        self.0.lock().unwrap().push(e);
    }
}
struct Now;
impl Clock for Now {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[test]
fn drives_a_containers_whole_lifecycle_through_the_application() {
    let (Some(sock), Some(image)) = (socket(), std::env::var("KAI_TEST_KIRI_IMAGE").ok()) else {
        return;
    };
    let name = format!("kai-test-life-{}", std::process::id());
    let (status, body) = raw(
        &sock,
        "POST",
        &format!("/containers/create?name={name}"),
        &format!(r#"{{"Image":"{image}","Cmd":["sleep","300"]}}"#),
    );
    assert_eq!(status, 201, "create: {body}");

    let engine = Arc::new(KiriEngine::at(&sock));
    let published = Arc::new(Collect(Default::default()));
    let service = ContainerService::new(
        engine.clone(),
        engine.clone(),
        published.clone(),
        Arc::new(Now),
    );
    let id = ContainerId::new(name.clone()).unwrap();
    let act = |action, confirmed| {
        service.perform_action(ContainerActionRequest {
            id: id.clone(),
            action,
            confirmed,
        })
    };
    let state = || engine.container(&id).unwrap().map(|c| c.state);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_eq!(state(), Some(ContainerState::Created));
        act(ContainerAction::Start, false).unwrap();
        assert_eq!(state(), Some(ContainerState::Running));
        assert!(
            matches!(
                act(ContainerAction::Remove, true),
                Err(AppError::NotAllowed(_))
            ),
            "a running container must not be removed"
        );
        act(ContainerAction::Pause, false).unwrap();
        assert_eq!(state(), Some(ContainerState::Paused));
        act(ContainerAction::Unpause, false).unwrap();
        act(ContainerAction::Restart, false).unwrap();
        assert_eq!(state(), Some(ContainerState::Running));
        act(ContainerAction::Stop, false).unwrap();
        assert_eq!(state(), Some(ContainerState::Exited));
        assert!(matches!(
            act(ContainerAction::Remove, false),
            Err(AppError::ConfirmationRequired { .. })
        ));
        act(ContainerAction::Remove, true).unwrap();
        assert_eq!(state(), None);
        assert_eq!(
            published
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|e| matches!(e, DomainEvent::ContainerActionPerformed { .. }))
                .count(),
            7
        );
    }));
    raw(&sock, "DELETE", &format!("/containers/{name}?force=1"), "");
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}
