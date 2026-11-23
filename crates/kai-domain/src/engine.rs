//! The container engine as a whole: what it is, and what it reports happening.

use crate::container::ContainerId;

/// What the engine says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineInfo {
    /// The engine product's own version (Kiri's), when it reports one.
    pub engine_version: String,
    /// The Docker Engine API version it speaks.
    pub api_version: String,
    pub os: String,
    pub arch: String,
}

/// Something the engine reports happening, as far as the platform cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineEvent {
    /// A container changed: created, started, died, removed… `action` is the
    /// engine's own word for it, kept for display and diagnostics.
    ContainerChanged { id: ContainerId, action: String },
    /// Images, volumes, networks and the rest: something changed that is not
    /// a container. Kept so consumers can refresh their other views.
    Other { kind: String, action: String },
}
