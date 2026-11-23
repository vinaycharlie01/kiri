//! Kai's native desktop views, on GPUI.
//!
//! A *driving adapter* in hexagonal terms: it turns clicks and keystrokes
//! into application use cases (`kai-application`) and renders their results.
//! It never reaches an adapter, a socket or the engine; the architecture
//! test in `kai-testkit` enforces that this crate does not depend on one.

pub mod model;
mod shell;
pub mod theme;

pub use shell::{
    Notice, Refresh, Services, Shell, ShellOptions, ShowContainers, ShowOverview, ToggleStopped,
    bind_keys,
};
pub use theme::ThemePreference;
