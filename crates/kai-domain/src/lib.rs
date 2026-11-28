//! Domain core of the Kai platform.
//!
//! Hexagonal architecture: this crate is the centre. It holds entities,
//! value objects, business rules, domain events and the *ports* (traits)
//! through which the application reaches the outside world. It depends on
//! `std` only — never on a UI framework, an HTTP client, a database or the
//! Kiri engine. Adapters implement the ports; the application layer
//! (`kai-application`) orchestrates use cases over them.
//!
//! See `docs/ai-platform/HEXAGONAL_ARCHITECTURE.md`.

#![forbid(unsafe_code)]

pub mod container;
pub mod engine;
pub mod event;
pub mod ports;

pub use container::{
    Container, ContainerAction, ContainerId, ContainerState, InvalidContainerId, PortMapping,
    Protocol,
};
pub use engine::{EngineEvent, EngineInfo};
pub use event::DomainEvent;
pub use ports::{
    Clock, ContainerRuntime, EngineEventSource, EngineLauncher, EventPublisher, RuntimeError,
};
