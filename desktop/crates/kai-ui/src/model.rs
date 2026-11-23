//! What the views show, decided without GPUI: navigation and the buttons a
//! container row offers. Pure, so it is unit-tested directly.

use kai_domain::{Container, ContainerAction};

/// A section the shell can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Overview,
    Containers,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Containers => "Containers",
        }
    }
}

/// One sidebar entry: a section that works, or a roadmap entry that says
/// which phase delivers it. Planned entries are shown, never clickable —
/// there are no placeholder screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavItem {
    Live(Section),
    Planned { label: &'static str, phase: u8 },
}

impl NavItem {
    pub fn label(self) -> &'static str {
        match self {
            Self::Live(s) => s.title(),
            Self::Planned { label, .. } => label,
        }
    }
}

/// The sidebar, in groups, as the product specification orders it.
pub const NAV: &[(&str, &[NavItem])] = &[
    ("", &[NavItem::Live(Section::Overview)]),
    (
        "AI Workspace",
        &[
            NavItem::Planned {
                label: "Agents",
                phase: 4,
            },
            NavItem::Planned {
                label: "Agent Sessions",
                phase: 4,
            },
            NavItem::Planned {
                label: "Workflows",
                phase: 7,
            },
            NavItem::Planned {
                label: "MCP Servers",
                phase: 5,
            },
            NavItem::Planned {
                label: "Models",
                phase: 4,
            },
            NavItem::Planned {
                label: "Tools",
                phase: 5,
            },
        ],
    ),
    (
        "Infrastructure",
        &[
            NavItem::Live(Section::Containers),
            NavItem::Planned {
                label: "Images",
                phase: 2,
            },
            NavItem::Planned {
                label: "Volumes",
                phase: 2,
            },
            NavItem::Planned {
                label: "Networks",
                phase: 2,
            },
            NavItem::Planned {
                label: "Kubernetes",
                phase: 2,
            },
            NavItem::Planned {
                label: "Machines",
                phase: 2,
            },
        ],
    ),
    (
        "Developer Tools",
        &[
            NavItem::Planned {
                label: "Terminal",
                phase: 2,
            },
            NavItem::Planned {
                label: "Logs",
                phase: 2,
            },
            NavItem::Planned {
                label: "Traces",
                phase: 8,
            },
            NavItem::Planned {
                label: "Metrics",
                phase: 8,
            },
        ],
    ),
    (
        "System",
        &[
            NavItem::Planned {
                label: "Settings",
                phase: 2,
            },
            NavItem::Planned {
                label: "Extensions",
                phase: 9,
            },
            NavItem::Planned {
                label: "Security",
                phase: 6,
            },
        ],
    ),
];

/// A button on a container row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowButton {
    Action(ContainerAction),
    /// The second step of a destructive action.
    Confirm(ContainerAction),
    CancelConfirm,
}

impl RowButton {
    pub fn label(self) -> &'static str {
        match self {
            Self::Action(a) => match a {
                ContainerAction::Start => "Start",
                ContainerAction::Stop => "Stop",
                ContainerAction::Restart => "Restart",
                ContainerAction::Pause => "Pause",
                ContainerAction::Unpause => "Resume",
                ContainerAction::Remove => "Remove",
            },
            Self::Confirm(_) => "Confirm remove",
            Self::CancelConfirm => "Cancel",
        }
    }

    pub fn is_danger(self) -> bool {
        matches!(self, Self::Confirm(_))
    }
}

/// The buttons a row shows: none while an action on it runs; Confirm and
/// Cancel while a destructive action awaits confirmation; otherwise one per
/// action the container's state allows.
pub fn row_buttons(
    c: &Container,
    busy: bool,
    confirming: Option<ContainerAction>,
) -> Vec<RowButton> {
    if busy {
        return Vec::new();
    }
    if let Some(action) = confirming.filter(|a| c.state.allows(*a)) {
        return vec![RowButton::Confirm(action), RowButton::CancelConfirm];
    }
    c.state
        .allowed_actions()
        .iter()
        .map(|&a| RowButton::Action(a))
        .collect()
}

/// Past tense for a completed action's notice.
pub fn done_notice(action: ContainerAction, name: &str) -> String {
    let verb = match action {
        ContainerAction::Start => "Started",
        ContainerAction::Stop => "Stopped",
        ContainerAction::Restart => "Restarted",
        ContainerAction::Pause => "Paused",
        ContainerAction::Unpause => "Resumed",
        ContainerAction::Remove => "Removed",
    };
    format!("{verb} {name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kai_domain::ContainerState;
    use kai_testkit::container;

    #[test]
    fn every_live_section_is_in_the_sidebar_once() {
        for s in [Section::Overview, Section::Containers] {
            let n = NAV
                .iter()
                .flat_map(|(_, items)| items.iter())
                .filter(|i| **i == NavItem::Live(s))
                .count();
            assert_eq!(n, 1, "{s:?}");
        }
    }

    #[test]
    fn rows_offer_what_the_state_allows() {
        let running = container("web", ContainerState::Running);
        assert_eq!(
            row_buttons(&running, false, None),
            [
                RowButton::Action(ContainerAction::Stop),
                RowButton::Action(ContainerAction::Restart),
                RowButton::Action(ContainerAction::Pause)
            ]
        );
    }

    #[test]
    fn a_busy_row_offers_nothing() {
        assert!(row_buttons(&container("web", ContainerState::Exited), true, None).is_empty());
    }

    #[test]
    fn remove_asks_for_confirmation_in_place() {
        let c = container("old", ContainerState::Exited);
        let buttons = row_buttons(&c, false, Some(ContainerAction::Remove));
        assert_eq!(
            buttons,
            [
                RowButton::Confirm(ContainerAction::Remove),
                RowButton::CancelConfirm
            ]
        );
        assert!(buttons[0].is_danger());
    }

    #[test]
    fn a_confirmation_the_state_no_longer_allows_is_dropped() {
        // Someone started the container while Confirm was showing.
        let c = container("old", ContainerState::Running);
        assert!(
            !row_buttons(&c, false, Some(ContainerAction::Remove))
                .iter()
                .any(|b| matches!(b, RowButton::Confirm(_)))
        );
    }

    #[test]
    fn notices_say_what_happened() {
        assert_eq!(done_notice(ContainerAction::Unpause, "web"), "Resumed web");
    }
}
