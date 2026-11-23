//! Application layer of the Kai platform: use cases.
//!
//! Each use case has an explicit input, output and error type, and depends on
//! ports only. Driving adapters (the native desktop, a CLI, later an HTTP or
//! MCP server) call these; they never call ports or adapters themselves.

#![forbid(unsafe_code)]

pub mod activity;
pub mod containers;
mod error;

pub use activity::ActivityFeed;
pub use containers::{
    ActionOutcome, ContainerActionRequest, ContainerList, ContainerService, EngineStatus,
    ListContainers, WatchNotice,
};
pub use error::AppError;
