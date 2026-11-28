//! Kai's driven adapter for the Kiri engine.
//!
//! Implements the domain's [`ContainerRuntime`] and [`EngineEventSource`]
//! ports by speaking Docker's Engine API to Kiri's daemon over its Unix
//! socket — the same public interface the docker CLI, Compose and Kiri's own
//! window use. Nothing in Kiri's engine is modified or linked: the platform
//! stays a client, so the engine can be replaced by any Docker-compatible
//! one without touching the application or domain.
//!
//! Unix only for now (macOS, Linux, and Kiri inside WSL2). On Windows the
//! engine is reached through kiri-win's named pipe `\\.\pipe\kiri`; that
//! transport is planned in docs/ai-platform/IMPLEMENTATION_ROADMAP.md.

#![forbid(unsafe_code)]

#[cfg(unix)]
mod http;
#[cfg(unix)]
mod launcher;
mod mapping;

#[cfg(unix)]
pub use launcher::KiriLauncher;
#[cfg(unix)]
pub use unix::KiriEngine;

#[cfg(unix)]
mod unix {
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};

    use kai_domain::{
        Container, ContainerAction, ContainerId, ContainerRuntime, EngineEvent, EngineEventSource,
        EngineInfo, RuntimeError,
    };

    use crate::http::{self, Response};
    use crate::mapping;

    /// Where the Linux `kiri` system service listens (contrib/linux/kiri.service).
    const SERVICE_SOCKET: &str = "/run/kiri/docker.sock";

    /// The Kiri engine, at one socket.
    #[derive(Clone, Debug)]
    pub struct KiriEngine {
        socket: PathBuf,
    }

    impl KiriEngine {
        pub fn at(socket: impl Into<PathBuf>) -> Self {
            Self {
                socket: socket.into(),
            }
        }

        /// The socket Kiri's own tools use: `KIRI_SOCKET`, else the Linux
        /// service's socket when it exists, else `~/.kiri/run/docker.sock`.
        pub fn discover() -> Option<Self> {
            Self::discover_with(
                std::env::var_os("KIRI_SOCKET").map(PathBuf::from),
                std::env::var_os("HOME").map(PathBuf::from),
                Path::new(SERVICE_SOCKET).exists(),
            )
        }

        fn discover_with(
            env_socket: Option<PathBuf>,
            home: Option<PathBuf>,
            service_exists: bool,
        ) -> Option<Self> {
            if let Some(s) = env_socket {
                return Some(Self::at(s));
            }
            if cfg!(target_os = "linux") && service_exists {
                return Some(Self::at(SERVICE_SOCKET));
            }
            home.map(|h| Self::at(h.join(".kiri/run/docker.sock")))
        }

        pub fn socket(&self) -> &Path {
            &self.socket
        }

        fn call(&self, method: &str, path: &str) -> Result<Response, RuntimeError> {
            http::request(&self.socket, method, path).map_err(|e| self.io_error(e))
        }

        fn io_error(&self, e: std::io::Error) -> RuntimeError {
            use std::io::ErrorKind::*;
            match e.kind() {
                NotFound | ConnectionRefused | PermissionDenied => {
                    RuntimeError::Unreachable(format!("{} ({e})", self.socket.display()))
                }
                _ => RuntimeError::Engine(format!("talking to the engine: {e}")),
            }
        }

        fn ok(r: Response) -> Result<Response, RuntimeError> {
            if (200..300).contains(&r.status) {
                Ok(r)
            } else {
                Err(mapping::status_error(r.status, r.message()))
            }
        }
    }

    impl ContainerRuntime for KiriEngine {
        fn engine_info(&self) -> Result<EngineInfo, RuntimeError> {
            mapping::engine_info(&Self::ok(self.call("GET", "/version")?)?.json())
        }

        fn list_containers(&self, include_stopped: bool) -> Result<Vec<Container>, RuntimeError> {
            let path = if include_stopped {
                "/containers/json?all=1"
            } else {
                "/containers/json"
            };
            mapping::containers(&Self::ok(self.call("GET", path)?)?.json())
        }

        fn container(&self, id: &ContainerId) -> Result<Option<Container>, RuntimeError> {
            // Inspect resolves a name or short ID to the full ID exactly
            // (the list's `name` filter is a regex, its `id` filter a prefix).
            let inspected = self.call("GET", &format!("/containers/{id}/json"))?;
            if inspected.status == 404 {
                return Ok(None);
            }
            let full = Self::ok(inspected)?.json()["Id"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| RuntimeError::Engine("inspect answer has no Id".into()))?;
            let full = ContainerId::new(full).map_err(|e| RuntimeError::Engine(e.to_string()))?;
            // Hex only, so it needs no escaping inside the filter.
            let filter = format!("%7B%22id%22%3A%5B%22{full}%22%5D%7D");
            let listed = mapping::containers(
                &Self::ok(self.call("GET", &format!("/containers/json?all=1&filters={filter}"))?)?
                    .json(),
            )?;
            Ok(listed.into_iter().find(|c| c.id == full))
        }

        fn perform(&self, id: &ContainerId, action: ContainerAction) -> Result<(), RuntimeError> {
            let (method, path) = match action {
                // Never `force`: the application only removes stopped containers.
                ContainerAction::Remove => ("DELETE", format!("/containers/{id}")),
                a => ("POST", format!("/containers/{id}/{}", a.verb())),
            };
            let r = self.call(method, &path)?;
            // 304: already in the requested state, which is what was asked for.
            if r.status == 304 {
                Ok(())
            } else {
                Self::ok(r).map(|_| ())
            }
        }
    }

    impl EngineEventSource for KiriEngine {
        fn follow(
            &self,
            on_event: &mut dyn FnMut(EngineEvent) -> ControlFlow<()>,
        ) -> Result<(), RuntimeError> {
            let mut on_line = |line: &[u8]| match mapping::event(line) {
                Some(e) => on_event(e),
                None => ControlFlow::Continue(()),
            };
            match http::stream_lines(&self.socket, "/events", &mut on_line)
                .map_err(|e| self.io_error(e))?
            {
                None => Ok(()),
                Some(r) => Err(mapping::status_error(r.status, r.message())),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        use std::sync::{Arc, Mutex};
        use std::thread::JoinHandle;

        const ID: &str = "4f1c0a5e2b3d4c5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6";

        /// A fake engine on a real Unix socket: answers each connection with
        /// the next scripted response and records the request lines.
        struct Scripted {
            dir: PathBuf,
            seen: Arc<Mutex<Vec<String>>>,
            server: Option<JoinHandle<()>>,
        }

        impl Scripted {
            fn new(responses: Vec<String>) -> Self {
                static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                let dir = std::env::temp_dir().join(format!(
                    "kai-kiri-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&dir).unwrap();
                let listener = UnixListener::bind(dir.join("docker.sock")).unwrap();
                let seen = Arc::new(Mutex::new(Vec::new()));
                let log = seen.clone();
                let server = std::thread::spawn(move || {
                    for response in responses {
                        let (conn, _) = listener.accept().unwrap();
                        let mut r = BufReader::new(conn.try_clone().unwrap());
                        let mut line = String::new();
                        r.read_line(&mut line).unwrap();
                        log.lock().unwrap().push(line.trim_end().to_owned());
                        loop {
                            let mut h = String::new();
                            if r.read_line(&mut h).unwrap() == 0 || h == "\r\n" {
                                break;
                            }
                        }
                        let mut conn = conn;
                        conn.write_all(response.as_bytes()).unwrap();
                    }
                });
                Self {
                    dir,
                    seen,
                    server: Some(server),
                }
            }

            fn engine(&self) -> KiriEngine {
                KiriEngine::at(self.dir.join("docker.sock"))
            }

            fn requests(mut self) -> Vec<String> {
                self.server.take().unwrap().join().unwrap();
                self.seen.lock().unwrap().clone()
            }
        }

        impl Drop for Scripted {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        fn json(status: &str, body: &str) -> String {
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
        }

        fn chunked(status: &str, chunks: &[&str]) -> String {
            let mut s = format!("HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\n\r\n");
            for c in chunks {
                s.push_str(&format!("{:x}\r\n{c}\r\n", c.len()));
            }
            s + "0\r\n\r\n"
        }

        fn id(s: &str) -> ContainerId {
            ContainerId::new(s).unwrap()
        }

        #[test]
        fn lists_containers_including_stopped_ones() {
            let body = format!(
                r#"[{{"Id":"{ID}","Names":["/web"],"Image":"nginx","State":"exited","Status":"Exited (0)"}}]"#
            );
            let s = Scripted::new(vec![json("200 OK", &body)]);
            let list = s.engine().list_containers(true).unwrap();
            assert_eq!(list[0].name, "web");
            assert_eq!(s.requests(), ["GET /containers/json?all=1 HTTP/1.1"]);
        }

        #[test]
        fn reads_chunked_bodies() {
            let s = Scripted::new(vec![chunked(
                "200 OK",
                &[
                    r#"{"KiriVersion":"0.2.3","#,
                    r#""ApiVersion":"1.47","Os":"linux","Arch":"amd64"}"#,
                ],
            )]);
            assert_eq!(s.engine().engine_info().unwrap().engine_version, "0.2.3");
        }

        #[test]
        fn looks_a_container_up_by_name_through_inspect_then_the_exact_id() {
            let listed = format!(r#"[{{"Id":"{ID}","Names":["/web"],"State":"running"}}]"#);
            let s = Scripted::new(vec![
                json("200 OK", &format!(r#"{{"Id":"{ID}"}}"#)),
                json("200 OK", &listed),
            ]);
            let c = s.engine().container(&id("web")).unwrap().unwrap();
            assert_eq!(c.id.as_str(), ID);
            let reqs = s.requests();
            assert_eq!(reqs[0], "GET /containers/web/json HTTP/1.1");
            assert_eq!(
                reqs[1],
                format!(
                    "GET /containers/json?all=1&filters=%7B%22id%22%3A%5B%22{ID}%22%5D%7D HTTP/1.1"
                )
            );
        }

        #[test]
        fn a_missing_container_is_none_not_an_error() {
            let s = Scripted::new(vec![json(
                "404 Not Found",
                r#"{"message":"No such container: ghost"}"#,
            )]);
            assert_eq!(s.engine().container(&id("ghost")).unwrap(), None);
        }

        #[test]
        fn actions_use_dockers_endpoints_and_remove_never_forces() {
            let s = Scripted::new(vec![
                "HTTP/1.1 204 No Content\r\n\r\n".into(),
                "HTTP/1.1 204 No Content\r\n\r\n".into(),
                "HTTP/1.1 204 No Content\r\n\r\n".into(),
            ]);
            let e = s.engine();
            e.perform(&id("web"), ContainerAction::Stop).unwrap();
            e.perform(&id("web"), ContainerAction::Unpause).unwrap();
            e.perform(&id("web"), ContainerAction::Remove).unwrap();
            assert_eq!(
                s.requests(),
                [
                    "POST /containers/web/stop HTTP/1.1",
                    "POST /containers/web/unpause HTTP/1.1",
                    "DELETE /containers/web HTTP/1.1"
                ]
            );
        }

        #[test]
        fn already_in_that_state_is_success() {
            let s = Scripted::new(vec!["HTTP/1.1 304 Not Modified\r\n\r\n".into()]);
            s.engine()
                .perform(&id("web"), ContainerAction::Start)
                .unwrap();
        }

        #[test]
        fn engine_refusals_keep_the_engines_message() {
            let s = Scripted::new(vec![
                json(
                    "409 Conflict",
                    r#"{"message":"You cannot remove a running container"}"#,
                ),
                json(
                    "500 Internal Server Error",
                    r#"{"message":"port 80 is already in use"}"#,
                ),
            ]);
            let e = s.engine();
            assert_eq!(
                e.perform(&id("web"), ContainerAction::Remove),
                Err(RuntimeError::Conflict(
                    "You cannot remove a running container".into()
                ))
            );
            assert_eq!(
                e.perform(&id("web"), ContainerAction::Start),
                Err(RuntimeError::Engine("port 80 is already in use".into()))
            );
        }

        #[test]
        fn a_missing_socket_is_unreachable() {
            let e = KiriEngine::at("/nonexistent/kai-test/docker.sock");
            assert!(matches!(e.engine_info(), Err(RuntimeError::Unreachable(_))));
        }

        #[test]
        fn follows_events_across_chunk_boundaries_until_told_to_stop() {
            let a = format!(r#"{{"Type":"container","Action":"start","Actor":{{"ID":"{ID}"}}}}"#);
            let (first, second) = a.split_at(20);
            let s = Scripted::new(vec![chunked(
                "200 OK",
                &[
                    first,
                    &format!("{second}\n")[..],
                    "{\"Type\":\"image\",\"Action\":\"pull\"}\n",
                    "{\"Type\":\"volume\",\"Action\":\"create\"}\n",
                ],
            )]);
            let mut seen = Vec::new();
            s.engine()
                .follow(&mut |e| {
                    seen.push(e);
                    if seen.len() == 2 {
                        ControlFlow::Break(())
                    } else {
                        ControlFlow::Continue(())
                    }
                })
                .unwrap();
            assert_eq!(
                seen,
                [
                    EngineEvent::ContainerChanged {
                        id: id(ID),
                        action: "start".into()
                    },
                    EngineEvent::Other {
                        kind: "image".into(),
                        action: "pull".into()
                    }
                ]
            );
        }

        #[test]
        fn discovery_follows_kiris_own_order() {
            let home = Some(PathBuf::from("/home/me"));
            assert_eq!(
                KiriEngine::discover_with(Some("/tmp/x.sock".into()), home.clone(), true)
                    .unwrap()
                    .socket(),
                Path::new("/tmp/x.sock")
            );
            let expected = if cfg!(target_os = "linux") {
                SERVICE_SOCKET
            } else {
                "/home/me/.kiri/run/docker.sock"
            };
            assert_eq!(
                KiriEngine::discover_with(None, home.clone(), true)
                    .unwrap()
                    .socket(),
                Path::new(expected)
            );
            assert_eq!(
                KiriEngine::discover_with(None, home, false)
                    .unwrap()
                    .socket(),
                Path::new("/home/me/.kiri/run/docker.sock")
            );
            assert!(KiriEngine::discover_with(None, None, false).is_none());
        }
    }
}
