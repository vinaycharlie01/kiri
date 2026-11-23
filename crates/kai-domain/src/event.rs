//! Domain events: important state changes the platform itself caused,
//! published through the [`EventPublisher`](crate::ports::EventPublisher)
//! port for activity feeds, audit and observability.

use std::time::SystemTime;

use crate::container::{ContainerAction, ContainerId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DomainEvent {
    /// A lifecycle action was performed on a container and the engine accepted it.
    ContainerActionPerformed {
        id: ContainerId,
        name: String,
        action: ContainerAction,
        at: SystemTime,
    },
    /// A lifecycle action was requested but failed or was refused.
    ContainerActionFailed {
        id: ContainerId,
        action: ContainerAction,
        reason: String,
        at: SystemTime,
    },
}

impl DomainEvent {
    pub fn at(&self) -> SystemTime {
        match self {
            Self::ContainerActionPerformed { at, .. } | Self::ContainerActionFailed { at, .. } => {
                *at
            }
        }
    }

    /// One line for an activity feed or a log.
    pub fn summary(&self) -> String {
        match self {
            Self::ContainerActionPerformed { name, action, .. } => {
                format!("{} {name}: done", capitalized(action.verb()))
            }
            Self::ContainerActionFailed {
                id, action, reason, ..
            } => format!("{} {}: {reason}", capitalized(action.verb()), id.short()),
        }
    }
}

fn capitalized(word: &str) -> String {
    let mut c = word.chars();
    c.next()
        .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_read_as_activity_lines() {
        let id = ContainerId::new("web").unwrap();
        let ok = DomainEvent::ContainerActionPerformed {
            id: id.clone(),
            name: "web".into(),
            action: ContainerAction::Stop,
            at: SystemTime::UNIX_EPOCH,
        };
        assert_eq!(ok.summary(), "Stop web: done");
        let failed = DomainEvent::ContainerActionFailed {
            id,
            action: ContainerAction::Start,
            reason: "port 80 is taken".into(),
            at: SystemTime::UNIX_EPOCH,
        };
        assert_eq!(failed.summary(), "Start web: port 80 is taken");
    }
}
