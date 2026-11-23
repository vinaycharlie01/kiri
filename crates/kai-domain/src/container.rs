//! Containers as the platform sees them: identity, lifecycle state, and the
//! rules for which lifecycle actions are valid in which state.
//!
//! The rules here are the platform's, not the engine's. The engine stays the
//! authority on what actually happened; these rules stop the application from
//! asking for actions that make no sense (unpausing a stopped container) or
//! that would be destructive without an explicit step (removing a running
//! container, which the engine would only do with `force`).

use std::fmt;

/// A container's identity: an engine ID (hex) or a container name.
///
/// Validated on construction, because it ends up in engine API paths: only
/// characters Docker allows in IDs and names are accepted, so a value can
/// never smuggle `/`, `?`, `%` or whitespace into a request.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContainerId(String);

/// Why a string is not a valid [`ContainerId`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidContainerId(pub String);

impl fmt::Display for InvalidContainerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a container ID or name: {:?}", self.0)
    }
}

impl std::error::Error for InvalidContainerId {}

impl ContainerId {
    /// Longest accepted value. Docker IDs are 64 hex characters; names are
    /// short in practice. The bound keeps hostile input from growing paths.
    pub const MAX_LEN: usize = 128;

    pub fn new(value: impl Into<String>) -> Result<Self, InvalidContainerId> {
        let value = value.into();
        // Docker's name pattern: [a-zA-Z0-9][a-zA-Z0-9_.-]*. IDs are a subset.
        let mut bytes = value.bytes();
        let first_ok = bytes.next().is_some_and(|b| b.is_ascii_alphanumeric());
        let rest_ok = bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
        if first_ok && rest_ok && value.len() <= Self::MAX_LEN {
            Ok(Self(value))
        } else {
            Err(InvalidContainerId(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The 12-character form people see in `docker ps`.
    pub fn short(&self) -> &str {
        let is_hex_id = self.0.len() == 64 && self.0.bytes().all(|b| b.is_ascii_hexdigit());
        if is_hex_id { &self.0[..12] } else { &self.0 }
    }
}

impl fmt::Display for ContainerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A container's lifecycle state, as Docker-compatible engines report it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Removing,
    Exited,
    Dead,
    /// A state this version does not know. No action is allowed on it: the
    /// platform will not guess what is safe.
    Unknown(String),
}

impl ContainerState {
    /// Parses the engine's state word (`running`, `exited`, …).
    pub fn parse(word: &str) -> Self {
        match word.to_ascii_lowercase().as_str() {
            "created" => Self::Created,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "restarting" => Self::Restarting,
            "removing" => Self::Removing,
            "exited" => Self::Exited,
            "dead" => Self::Dead,
            _ => Self::Unknown(word.to_owned()),
        }
    }

    /// Whether the container's processes exist (running, paused or restarting).
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Running | Self::Paused | Self::Restarting)
    }

    /// The lifecycle actions valid in this state, in the order a UI offers them.
    pub fn allowed_actions(&self) -> &'static [ContainerAction] {
        use ContainerAction::*;
        match self {
            Self::Created => &[Start, Remove],
            Self::Running => &[Stop, Restart, Pause],
            Self::Paused => &[Unpause, Stop],
            Self::Restarting => &[Stop],
            Self::Exited => &[Start, Restart, Remove],
            Self::Dead => &[Remove],
            Self::Removing | Self::Unknown(_) => &[],
        }
    }

    pub fn allows(&self, action: ContainerAction) -> bool {
        self.allowed_actions().contains(&action)
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Restarting => "restarting",
            Self::Removing => "removing",
            Self::Exited => "exited",
            Self::Dead => "dead",
            Self::Unknown(s) => s,
        }
    }
}

impl fmt::Display for ContainerState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A lifecycle action on one container.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
    Remove,
}

impl ContainerAction {
    pub const ALL: [ContainerAction; 6] = [
        Self::Start,
        Self::Stop,
        Self::Restart,
        Self::Pause,
        Self::Unpause,
        Self::Remove,
    ];

    /// Destructive actions lose data and need explicit confirmation.
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Remove)
    }

    pub fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Pause => "pause",
            Self::Unpause => "unpause",
            Self::Remove => "remove",
        }
    }
}

impl fmt::Display for ContainerAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.verb())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
    Sctp,
}

impl Protocol {
    pub fn parse(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "tcp" => Some(Self::Tcp),
            "udp" => Some(Self::Udp),
            "sctp" => Some(Self::Sctp),
            _ => None,
        }
    }
}

/// One port a container exposes, and where it is published on the host.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PortMapping {
    pub container_port: u16,
    pub host_port: Option<u16>,
    pub host_ip: Option<String>,
    pub protocol: Protocol,
}

impl fmt::Display for PortMapping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let proto = match self.protocol {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
            Protocol::Sctp => "sctp",
        };
        match self.host_port {
            Some(host) => write!(f, "{host}→{}/{proto}", self.container_port),
            None => write!(f, "{}/{proto}", self.container_port),
        }
    }
}

/// A container entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Container {
    pub id: ContainerId,
    /// The name without Docker's leading `/`.
    pub name: String,
    pub image: String,
    pub state: ContainerState,
    /// The engine's human status line ("Up 5 minutes", "Exited (0) 2 hours ago").
    pub status: String,
    pub ports: Vec<PortMapping>,
    /// Seconds since the Unix epoch.
    pub created_unix: i64,
    /// The Compose project, when the container belongs to one.
    pub compose_project: Option<String>,
}

impl Container {
    /// Checks that `action` is valid now; the error says why not.
    pub fn check(&self, action: ContainerAction) -> Result<(), ActionNotAllowed> {
        if self.state.allows(action) {
            Ok(())
        } else {
            Err(ActionNotAllowed {
                action,
                state: self.state.clone(),
            })
        }
    }
}

/// An action that the container's current state does not allow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionNotAllowed {
    pub action: ContainerAction,
    pub state: ContainerState,
}

impl fmt::Display for ActionNotAllowed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot {} a container that is {}",
            self.action, self.state
        )?;
        if self.action == ContainerAction::Remove && self.state.is_active() {
            f.write_str(" (stop it first)")?;
        }
        Ok(())
    }
}

impl std::error::Error for ActionNotAllowed {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_names_are_accepted() {
        for ok in ["web", "web-1", "my_app.v2", "a", &"f".repeat(64)] {
            assert!(ContainerId::new(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn values_that_could_alter_an_api_path_are_rejected() {
        for bad in [
            "",
            "-web",
            ".hidden",
            "a/b",
            "../x",
            "a?force=1",
            "a%2F",
            "a b",
            "a\nb",
            "é",
            &"a".repeat(129),
        ] {
            assert!(ContainerId::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn short_form_only_shortens_full_hex_ids() {
        let id = ContainerId::new("0123456789ab".repeat(5) + "cdef").unwrap();
        assert_eq!(id.short(), "0123456789ab");
        assert_eq!(ContainerId::new("web").unwrap().short(), "web");
    }

    #[test]
    fn states_parse_case_insensitively_and_keep_unknown_words() {
        assert_eq!(ContainerState::parse("Running"), ContainerState::Running);
        assert_eq!(
            ContainerState::parse("hibernating"),
            ContainerState::Unknown("hibernating".into())
        );
    }

    #[test]
    fn running_containers_cannot_be_removed_or_started() {
        let s = ContainerState::Running;
        assert!(!s.allows(ContainerAction::Remove));
        assert!(!s.allows(ContainerAction::Start));
        assert!(s.allows(ContainerAction::Stop));
    }

    #[test]
    fn every_state_allows_exactly_its_listed_actions() {
        use ContainerAction::*;
        let cases = [
            (ContainerState::Created, vec![Start, Remove]),
            (ContainerState::Running, vec![Stop, Restart, Pause]),
            (ContainerState::Paused, vec![Unpause, Stop]),
            (ContainerState::Restarting, vec![Stop]),
            (ContainerState::Exited, vec![Start, Restart, Remove]),
            (ContainerState::Dead, vec![Remove]),
            (ContainerState::Removing, vec![]),
            (ContainerState::Unknown("x".into()), vec![]),
        ];
        for (state, allowed) in cases {
            for action in ContainerAction::ALL {
                assert_eq!(
                    state.allows(action),
                    allowed.contains(&action),
                    "{state} / {action}"
                );
            }
        }
    }

    #[test]
    fn only_remove_is_destructive() {
        let destructive: Vec<_> = ContainerAction::ALL
            .into_iter()
            .filter(|a| a.is_destructive())
            .collect();
        assert_eq!(destructive, [ContainerAction::Remove]);
    }

    #[test]
    fn a_refused_remove_says_to_stop_first() {
        let c = Container {
            id: ContainerId::new("web").unwrap(),
            name: "web".into(),
            image: "nginx".into(),
            state: ContainerState::Running,
            status: "Up".into(),
            ports: vec![],
            created_unix: 0,
            compose_project: None,
        };
        let err = c.check(ContainerAction::Remove).unwrap_err();
        assert_eq!(
            err.to_string(),
            "cannot remove a container that is running (stop it first)"
        );
        assert!(c.check(ContainerAction::Stop).is_ok());
    }

    #[test]
    fn ports_display_like_docker_ps() {
        let p = PortMapping {
            container_port: 80,
            host_port: Some(8080),
            host_ip: None,
            protocol: Protocol::Tcp,
        };
        assert_eq!(p.to_string(), "8080→80/tcp");
        let p = PortMapping {
            container_port: 53,
            host_port: None,
            host_ip: None,
            protocol: Protocol::Udp,
        };
        assert_eq!(p.to_string(), "53/udp");
    }
}
