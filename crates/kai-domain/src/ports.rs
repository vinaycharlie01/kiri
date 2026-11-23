//! Ports: the traits through which the application reaches the outside world.
//!
//! Each port is small and has one responsibility. Adapters implement them
//! (`kai-adapter-kiri` for the Kiri engine, `kai-testkit` for deterministic
//! fakes); the composition root of each driving adapter (the desktop app, a
//! CLI) chooses which.
//!
//! Ports are synchronous and blocking. The Kiri engine's API is reached over
//! a local socket in milliseconds, and the existing Kiri codebase is
//! thread-based rather than async; driving adapters call ports from
//! background threads, never a UI thread. I/O-bound ports that need high
//! concurrency (LLM streaming, MCP) will be introduced as async ports in
//! their own phase — see `docs/ai-platform/HEXAGONAL_ARCHITECTURE.md`, ADR-3.

use std::fmt;
use std::ops::ControlFlow;
use std::time::SystemTime;

use crate::container::{Container, ContainerAction, ContainerId};
use crate::engine::{EngineEvent, EngineInfo};
use crate::event::DomainEvent;

/// Why a runtime port call failed, normalised across engines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    /// The engine could not be reached at all (not running, socket missing).
    Unreachable(String),
    /// The container (or other object) does not exist.
    NotFound(String),
    /// The engine refused because of the object's state (Docker's 409).
    Conflict(String),
    /// The engine refused the request for another reason (4xx).
    Rejected(String),
    /// The engine failed (5xx) or answered something this adapter cannot read.
    Engine(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(m) => write!(f, "engine not reachable: {m}"),
            Self::NotFound(m) | Self::Conflict(m) | Self::Rejected(m) | Self::Engine(m) => {
                f.write_str(m)
            }
        }
    }
}

impl std::error::Error for RuntimeError {}

/// A container engine: listing containers and changing their lifecycle.
pub trait ContainerRuntime: Send + Sync {
    /// The engine's identity; also the cheapest way to see it is reachable.
    fn engine_info(&self) -> Result<EngineInfo, RuntimeError>;

    /// Containers, including stopped ones when `include_stopped`.
    fn list_containers(&self, include_stopped: bool) -> Result<Vec<Container>, RuntimeError>;

    /// One container by ID or name; `Ok(None)` when there is none.
    fn container(&self, id: &ContainerId) -> Result<Option<Container>, RuntimeError>;

    /// Performs `action`. An engine reporting the container already in the
    /// requested state (Docker's 304) is success. `Remove` never forces:
    /// the application only asks for it when the state allows it.
    fn perform(&self, id: &ContainerId, action: ContainerAction) -> Result<(), RuntimeError>;
}

/// The engine's stream of things happening.
pub trait EngineEventSource: Send + Sync {
    /// Blocks, calling `on_event` for each event until it returns
    /// `ControlFlow::Break`, or the stream ends (`Ok`) or fails (`Err`).
    fn follow(
        &self,
        on_event: &mut dyn FnMut(EngineEvent) -> ControlFlow<()>,
    ) -> Result<(), RuntimeError>;
}

/// Where domain events go: an activity feed, an audit log, telemetry.
/// Publishing never fails the use case that caused the event.
pub trait EventPublisher: Send + Sync {
    fn publish(&self, event: DomainEvent);
}

/// The time, so use cases and their tests do not read the system clock.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}
