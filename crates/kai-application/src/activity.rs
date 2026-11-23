//! RecentActivity: a bounded, in-memory read model of the domain events the
//! platform published — what the desktop's activity feed shows.
//!
//! It is an [`EventPublisher`] (so a composition root can hand it to the use
//! cases) and a query (so a driving adapter can read it), with no
//! infrastructure: durable audit storage is a separate adapter, later.

use std::collections::VecDeque;
use std::sync::Mutex;

use kai_domain::{DomainEvent, EventPublisher};

#[derive(Debug)]
pub struct ActivityFeed {
    capacity: usize,
    events: Mutex<VecDeque<DomainEvent>>,
}

impl ActivityFeed {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            events: Mutex::new(VecDeque::new()),
        }
    }

    /// The newest events first, at most `limit`.
    pub fn recent(&self, limit: usize) -> Vec<DomainEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }
}

impl EventPublisher for ActivityFeed {
    fn publish(&self, event: DomainEvent) {
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if events.len() == self.capacity {
            events.pop_front();
        }
        events.push_back(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kai_domain::{ContainerAction, ContainerId};
    use std::time::SystemTime;

    fn ev(n: &str) -> DomainEvent {
        DomainEvent::ContainerActionPerformed {
            id: ContainerId::new(n).unwrap(),
            name: n.into(),
            action: ContainerAction::Start,
            at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn keeps_the_newest_events_newest_first() {
        let feed = ActivityFeed::new(2);
        for n in ["a", "b", "c"] {
            feed.publish(ev(n));
        }
        assert_eq!(feed.recent(10), [ev("c"), ev("b")]);
        assert_eq!(feed.recent(1), [ev("c")]);
    }
}
