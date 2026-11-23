//! Container use cases: EngineStatus, ListContainers, PerformContainerAction
//! and WatchContainerChanges.

use std::ops::ControlFlow;
use std::sync::Arc;

use kai_domain::{
    Clock, Container, ContainerAction, ContainerId, ContainerRuntime, ContainerState, DomainEvent,
    EngineEvent, EngineEventSource, EngineInfo, EventPublisher, RuntimeError,
};

use crate::AppError;

/// Whether the engine answers, and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineStatus {
    Connected(EngineInfo),
    /// Not reachable; the reason is for display.
    Unreachable(String),
}

/// Input of the ListContainers use case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListContainers {
    pub include_stopped: bool,
}

/// Output of ListContainers: active containers first, then by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContainerList {
    pub containers: Vec<Container>,
    pub running: usize,
}

/// Input of the PerformContainerAction use case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerActionRequest {
    pub id: ContainerId,
    pub action: ContainerAction,
    /// The person explicitly confirmed. Required for destructive actions.
    pub confirmed: bool,
}

/// Output of PerformContainerAction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionOutcome {
    pub id: ContainerId,
    pub name: String,
    pub action: ContainerAction,
}

/// What WatchContainerChanges tells its caller: coarse on purpose, so views
/// re-query through ListContainers rather than patch state from events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchNotice {
    ContainersChanged { id: ContainerId, action: String },
    OtherChanged { kind: String },
}

/// The container use cases, over injected ports.
pub struct ContainerService {
    runtime: Arc<dyn ContainerRuntime>,
    events: Arc<dyn EngineEventSource>,
    publisher: Arc<dyn EventPublisher>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ContainerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerService").finish_non_exhaustive()
    }
}

impl ContainerService {
    pub fn new(
        runtime: Arc<dyn ContainerRuntime>,
        events: Arc<dyn EngineEventSource>,
        publisher: Arc<dyn EventPublisher>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            runtime,
            events,
            publisher,
            clock,
        }
    }

    /// EngineStatus. Never fails: an unreachable engine is a status.
    pub fn engine_status(&self) -> EngineStatus {
        match self.runtime.engine_info() {
            Ok(info) => EngineStatus::Connected(info),
            Err(e) => EngineStatus::Unreachable(e.to_string()),
        }
    }

    /// ListContainers.
    pub fn list_containers(&self, input: ListContainers) -> Result<ContainerList, AppError> {
        let mut containers = self.runtime.list_containers(input.include_stopped)?;
        containers.sort_by(|a, b| {
            b.state
                .is_active()
                .cmp(&a.state.is_active())
                .then_with(|| a.name.cmp(&b.name))
        });
        let running = containers
            .iter()
            .filter(|c| c.state == ContainerState::Running)
            .count();
        Ok(ContainerList {
            containers,
            running,
        })
    }

    /// PerformContainerAction.
    ///
    /// Rules, in order: destructive actions need `confirmed`; the container
    /// must exist *now* (state is read fresh, never trusted from the caller);
    /// its current state must allow the action. Then the engine is asked, and
    /// the outcome is published as a domain event either way.
    pub fn perform_action(&self, req: ContainerActionRequest) -> Result<ActionOutcome, AppError> {
        let container = self
            .runtime
            .container(&req.id)?
            .ok_or_else(|| AppError::NotFound(req.id.clone()))?;
        if req.action.is_destructive() && !req.confirmed {
            return Err(AppError::ConfirmationRequired {
                action: req.action,
                name: container.name,
            });
        }
        container.check(req.action).map_err(AppError::NotAllowed)?;
        match self.runtime.perform(&req.id, req.action) {
            Ok(()) => {
                self.publisher
                    .publish(DomainEvent::ContainerActionPerformed {
                        id: req.id.clone(),
                        name: container.name.clone(),
                        action: req.action,
                        at: self.clock.now(),
                    });
                Ok(ActionOutcome {
                    id: req.id,
                    name: container.name,
                    action: req.action,
                })
            }
            Err(e) => {
                self.publisher.publish(DomainEvent::ContainerActionFailed {
                    id: req.id.clone(),
                    action: req.action,
                    reason: e.to_string(),
                    at: self.clock.now(),
                });
                Err(match e {
                    RuntimeError::NotFound(_) => AppError::NotFound(req.id),
                    e => AppError::Runtime(e),
                })
            }
        }
    }

    /// WatchContainerChanges: blocks, following the engine's events, until
    /// `on_notice` breaks or the stream ends. Reconnecting is the caller's
    /// choice (it knows whether anyone is still watching).
    pub fn watch_changes(
        &self,
        on_notice: &mut dyn FnMut(WatchNotice) -> ControlFlow<()>,
    ) -> Result<(), AppError> {
        self.events
            .follow(&mut |event| {
                on_notice(match event {
                    EngineEvent::ContainerChanged { id, action } => {
                        WatchNotice::ContainersChanged { id, action }
                    }
                    EngineEvent::Other { kind, .. } => WatchNotice::OtherChanged { kind },
                })
            })
            .map_err(AppError::Runtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kai_testkit::{FakeEvents, FakeRuntime, FixedClock, RecordingPublisher, container};

    struct Fixture {
        runtime: Arc<FakeRuntime>,
        events: Arc<FakeEvents>,
        published: Arc<RecordingPublisher>,
        service: ContainerService,
    }

    fn fixture(containers: Vec<Container>) -> Fixture {
        let runtime = Arc::new(FakeRuntime::with(containers));
        let events = Arc::new(FakeEvents::default());
        let published = Arc::new(RecordingPublisher::default());
        let service = ContainerService::new(
            runtime.clone(),
            events.clone(),
            published.clone(),
            Arc::new(FixedClock::epoch()),
        );
        Fixture {
            runtime,
            events,
            published,
            service,
        }
    }

    fn id(s: &str) -> ContainerId {
        ContainerId::new(s).unwrap()
    }

    fn request(name: &str, action: ContainerAction, confirmed: bool) -> ContainerActionRequest {
        ContainerActionRequest {
            id: id(name),
            action,
            confirmed,
        }
    }

    #[test]
    fn lists_active_containers_first_then_by_name() {
        let f = fixture(vec![
            container("zeta", ContainerState::Exited),
            container("beta", ContainerState::Running),
            container("alpha", ContainerState::Exited),
            container("gamma", ContainerState::Paused),
        ]);
        let list = f
            .service
            .list_containers(ListContainers {
                include_stopped: true,
            })
            .unwrap();
        let names: Vec<_> = list.containers.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["beta", "gamma", "alpha", "zeta"]);
        assert_eq!(list.running, 1);
    }

    #[test]
    fn listing_without_stopped_asks_the_engine_for_active_only() {
        let f = fixture(vec![
            container("a", ContainerState::Running),
            container("b", ContainerState::Exited),
        ]);
        let list = f
            .service
            .list_containers(ListContainers {
                include_stopped: false,
            })
            .unwrap();
        assert_eq!(list.containers.len(), 1);
    }

    #[test]
    fn an_unreachable_engine_is_a_status_not_an_error() {
        let f = fixture(vec![]);
        assert!(matches!(
            f.service.engine_status(),
            EngineStatus::Connected(_)
        ));
        f.runtime.set_reachable(false);
        assert!(
            matches!(f.service.engine_status(), EngineStatus::Unreachable(m) if m.contains("not reachable"))
        );
        assert!(matches!(
            f.service.list_containers(ListContainers {
                include_stopped: true
            }),
            Err(AppError::Runtime(RuntimeError::Unreachable(_)))
        ));
    }

    #[test]
    fn stopping_a_running_container_reaches_the_engine_and_publishes_an_event() {
        let f = fixture(vec![container("web", ContainerState::Running)]);
        let out = f
            .service
            .perform_action(request("web", ContainerAction::Stop, false))
            .unwrap();
        assert_eq!(out.name, "web");
        assert_eq!(f.runtime.calls(), [(id("web"), ContainerAction::Stop)]);
        assert_eq!(f.runtime.state_of("web"), Some(ContainerState::Exited));
        assert!(matches!(
            &f.published.events()[..],
            [DomainEvent::ContainerActionPerformed {
                action: ContainerAction::Stop,
                ..
            }]
        ));
    }

    #[test]
    fn remove_without_confirmation_is_refused_before_the_engine_is_asked() {
        let f = fixture(vec![container("old", ContainerState::Exited)]);
        let err = f
            .service
            .perform_action(request("old", ContainerAction::Remove, false))
            .unwrap_err();
        assert_eq!(
            err,
            AppError::ConfirmationRequired {
                action: ContainerAction::Remove,
                name: "old".into()
            }
        );
        assert!(f.runtime.calls().is_empty());
        assert!(f.published.events().is_empty());
    }

    #[test]
    fn confirmed_remove_of_a_stopped_container_removes_it() {
        let f = fixture(vec![container("old", ContainerState::Exited)]);
        f.service
            .perform_action(request("old", ContainerAction::Remove, true))
            .unwrap();
        assert_eq!(f.runtime.state_of("old"), None);
    }

    #[test]
    fn a_running_container_is_never_removed_even_when_confirmed() {
        let f = fixture(vec![container("web", ContainerState::Running)]);
        let err = f
            .service
            .perform_action(request("web", ContainerAction::Remove, true))
            .unwrap_err();
        assert!(matches!(err, AppError::NotAllowed(_)), "{err:?}");
        assert!(f.runtime.calls().is_empty());
    }

    #[test]
    fn state_is_read_fresh_so_a_stale_view_cannot_force_an_invalid_action() {
        // The UI saw "exited" and offers Start; meanwhile someone started it.
        let f = fixture(vec![container("web", ContainerState::Running)]);
        let err = f
            .service
            .perform_action(request("web", ContainerAction::Start, false))
            .unwrap_err();
        assert!(matches!(err, AppError::NotAllowed(ref e) if e.state == ContainerState::Running));
    }

    #[test]
    fn a_missing_container_is_not_found() {
        let f = fixture(vec![]);
        assert_eq!(
            f.service
                .perform_action(request("ghost", ContainerAction::Start, false)),
            Err(AppError::NotFound(id("ghost")))
        );
    }

    #[test]
    fn an_engine_failure_is_returned_and_published() {
        let f = fixture(vec![container("web", ContainerState::Exited)]);
        f.runtime
            .fail_next(RuntimeError::Engine("port 80 is already in use".into()));
        let err = f
            .service
            .perform_action(request("web", ContainerAction::Start, false))
            .unwrap_err();
        assert_eq!(
            err,
            AppError::Runtime(RuntimeError::Engine("port 80 is already in use".into()))
        );
        assert!(
            matches!(&f.published.events()[..], [DomainEvent::ContainerActionFailed { reason, .. }] if reason.contains("port 80"))
        );
    }

    #[test]
    fn a_container_gone_between_lookup_and_action_is_not_found() {
        let f = fixture(vec![container("web", ContainerState::Running)]);
        f.runtime
            .fail_next(RuntimeError::NotFound("No such container: web".into()));
        assert_eq!(
            f.service
                .perform_action(request("web", ContainerAction::Stop, false)),
            Err(AppError::NotFound(id("web")))
        );
    }

    #[test]
    fn watching_maps_engine_events_to_notices_and_stops_when_asked() {
        let f = fixture(vec![]);
        f.events.push(EngineEvent::ContainerChanged {
            id: id("web"),
            action: "start".into(),
        });
        f.events.push(EngineEvent::Other {
            kind: "image".into(),
            action: "pull".into(),
        });
        f.events.push(EngineEvent::ContainerChanged {
            id: id("db"),
            action: "die".into(),
        });
        let mut seen = Vec::new();
        f.service
            .watch_changes(&mut |n| {
                seen.push(n);
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
                WatchNotice::ContainersChanged {
                    id: id("web"),
                    action: "start".into()
                },
                WatchNotice::OtherChanged {
                    kind: "image".into()
                }
            ]
        );
    }
}
