//! Deterministic fakes of the domain's ports, for tests that must not need a
//! running engine, network access or credentials.
//!
//! The fakes behave like a Docker-compatible engine where it matters to
//! callers: actions change state, "already in that state" succeeds (Docker's
//! 304), removing an active container is a conflict, and unknown containers
//! are not found. Failures can be scripted.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::ops::ControlFlow;
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

use std::sync::Arc;

use kai_domain::{
    Clock, Container, ContainerAction, ContainerId, ContainerRuntime, ContainerState, DomainEvent,
    EngineEvent, EngineEventSource, EngineInfo, EngineLauncher, EventPublisher, RuntimeError,
};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A container named `name` (also its ID) in `state`, with plain defaults.
pub fn container(name: &str, state: ContainerState) -> Container {
    Container {
        id: ContainerId::new(name).expect("test container names are valid IDs"),
        name: name.to_owned(),
        image: "alpine:3.21".to_owned(),
        status: state.label().to_owned(),
        state,
        ports: Vec::new(),
        created_unix: 0,
        compose_project: None,
    }
}

#[derive(Debug, Default)]
struct RuntimeState {
    containers: Vec<Container>,
    unreachable: bool,
    fail_next: Option<RuntimeError>,
    calls: Vec<(ContainerId, ContainerAction)>,
}

/// An in-memory [`ContainerRuntime`].
#[derive(Debug, Default)]
pub struct FakeRuntime {
    state: Mutex<RuntimeState>,
}

impl FakeRuntime {
    pub fn with(containers: Vec<Container>) -> Self {
        Self {
            state: Mutex::new(RuntimeState {
                containers,
                ..Default::default()
            }),
        }
    }

    pub fn set_reachable(&self, reachable: bool) {
        lock(&self.state).unreachable = !reachable;
    }

    /// The next `perform` fails with `error` instead of acting.
    pub fn fail_next(&self, error: RuntimeError) {
        lock(&self.state).fail_next = Some(error);
    }

    /// Every `perform` the engine received, in order.
    pub fn calls(&self) -> Vec<(ContainerId, ContainerAction)> {
        lock(&self.state).calls.clone()
    }

    pub fn state_of(&self, name: &str) -> Option<ContainerState> {
        lock(&self.state)
            .containers
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.state.clone())
    }

    fn reachable(s: &RuntimeState) -> Result<(), RuntimeError> {
        if s.unreachable {
            Err(RuntimeError::Unreachable("fake engine is down".into()))
        } else {
            Ok(())
        }
    }
}

impl ContainerRuntime for FakeRuntime {
    fn engine_info(&self) -> Result<EngineInfo, RuntimeError> {
        Self::reachable(&lock(&self.state))?;
        Ok(EngineInfo {
            engine_version: "fake".into(),
            api_version: "1.47".into(),
            os: "linux".into(),
            arch: "amd64".into(),
        })
    }

    fn list_containers(&self, include_stopped: bool) -> Result<Vec<Container>, RuntimeError> {
        let s = lock(&self.state);
        Self::reachable(&s)?;
        Ok(s.containers
            .iter()
            .filter(|c| include_stopped || c.state.is_active())
            .cloned()
            .collect())
    }

    fn container(&self, id: &ContainerId) -> Result<Option<Container>, RuntimeError> {
        let s = lock(&self.state);
        Self::reachable(&s)?;
        Ok(s.containers
            .iter()
            .find(|c| &c.id == id || c.name == id.as_str())
            .cloned())
    }

    fn perform(&self, id: &ContainerId, action: ContainerAction) -> Result<(), RuntimeError> {
        let mut s = lock(&self.state);
        Self::reachable(&s)?;
        s.calls.push((id.clone(), action));
        if let Some(e) = s.fail_next.take() {
            return Err(e);
        }
        let i = s
            .containers
            .iter()
            .position(|c| &c.id == id)
            .ok_or_else(|| RuntimeError::NotFound(format!("No such container: {id}")))?;
        let current = s.containers[i].state.clone();
        let next = match (action, &current) {
            (ContainerAction::Remove, st) if st.is_active() => {
                return Err(RuntimeError::Conflict(format!(
                    "container {id} is {st}: stop it first"
                )));
            }
            (ContainerAction::Remove, _) => {
                s.containers.remove(i);
                return Ok(());
            }
            (ContainerAction::Start | ContainerAction::Restart | ContainerAction::Unpause, _) => {
                ContainerState::Running
            }
            (ContainerAction::Stop, _) => ContainerState::Exited,
            (ContainerAction::Pause, _) => ContainerState::Paused,
        };
        let c = &mut s.containers[i];
        c.status = next.label().to_owned();
        c.state = next;
        Ok(())
    }
}

/// A scripted [`EngineEventSource`]: delivers the queued events, then ends
/// the stream (or fails it, when told to).
#[derive(Debug, Default)]
pub struct FakeEvents {
    queue: Mutex<VecDeque<EngineEvent>>,
    end_with: Mutex<Option<RuntimeError>>,
}

impl FakeEvents {
    pub fn push(&self, event: EngineEvent) {
        lock(&self.queue).push_back(event);
    }

    pub fn end_with_error(&self, error: RuntimeError) {
        *lock(&self.end_with) = Some(error);
    }
}

impl EngineEventSource for FakeEvents {
    fn follow(
        &self,
        on_event: &mut dyn FnMut(EngineEvent) -> ControlFlow<()>,
    ) -> Result<(), RuntimeError> {
        loop {
            let Some(event) = lock(&self.queue).pop_front() else {
                break;
            };
            if on_event(event).is_break() {
                return Ok(());
            }
        }
        lock(&self.end_with).take().map_or(Ok(()), Err)
    }
}

/// An [`EventPublisher`] that keeps what it is given.
#[derive(Debug, Default)]
pub struct RecordingPublisher {
    events: Mutex<Vec<DomainEvent>>,
}

impl RecordingPublisher {
    pub fn events(&self) -> Vec<DomainEvent> {
        lock(&self.events).clone()
    }
}

impl EventPublisher for RecordingPublisher {
    fn publish(&self, event: DomainEvent) {
        lock(&self.events).push(event);
    }
}

/// A [`Clock`] that always says the same time.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub SystemTime);

impl FixedClock {
    pub fn epoch() -> Self {
        Self(SystemTime::UNIX_EPOCH)
    }
}

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// An [`EngineLauncher`] that "starts" a [`FakeRuntime`] by making it
/// reachable, or fails as told. Counts its launches.
#[derive(Debug)]
pub struct FakeLauncher {
    starts: Option<Arc<FakeRuntime>>,
    failure: Option<String>,
    launches: Mutex<usize>,
}

impl FakeLauncher {
    pub fn starting(runtime: Arc<FakeRuntime>) -> Self {
        Self {
            starts: Some(runtime),
            failure: None,
            launches: Mutex::new(0),
        }
    }

    pub fn failing(reason: &str) -> Self {
        Self {
            starts: None,
            failure: Some(reason.to_owned()),
            launches: Mutex::new(0),
        }
    }

    pub fn launches(&self) -> usize {
        *lock(&self.launches)
    }
}

impl EngineLauncher for FakeLauncher {
    fn launch(&self) -> Result<(), RuntimeError> {
        *lock(&self.launches) += 1;
        if let Some(reason) = &self.failure {
            return Err(RuntimeError::Engine(reason.clone()));
        }
        if let Some(rt) = &self.starts {
            rt.set_reachable(true);
        }
        Ok(())
    }

    fn describe(&self) -> String {
        "fake engine".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ContainerId {
        ContainerId::new(s).unwrap()
    }

    #[test]
    fn the_fake_engine_treats_repeat_actions_as_done_and_refuses_removing_active_ones() {
        let rt = FakeRuntime::with(vec![container("web", ContainerState::Running)]);
        rt.perform(&id("web"), ContainerAction::Start).unwrap();
        assert!(matches!(
            rt.perform(&id("web"), ContainerAction::Remove),
            Err(RuntimeError::Conflict(_))
        ));
        rt.perform(&id("web"), ContainerAction::Stop).unwrap();
        rt.perform(&id("web"), ContainerAction::Remove).unwrap();
        assert!(matches!(
            rt.perform(&id("web"), ContainerAction::Stop),
            Err(RuntimeError::NotFound(_))
        ));
    }

    #[test]
    fn scripted_event_streams_can_fail_at_the_end() {
        let ev = FakeEvents::default();
        ev.push(EngineEvent::Other {
            kind: "image".into(),
            action: "pull".into(),
        });
        ev.end_with_error(RuntimeError::Unreachable("gone".into()));
        let mut n = 0;
        assert!(
            ev.follow(&mut |_| {
                n += 1;
                ControlFlow::Continue(())
            })
            .is_err()
        );
        assert_eq!(n, 1);
    }
}
