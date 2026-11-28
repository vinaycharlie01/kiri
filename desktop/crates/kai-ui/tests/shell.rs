//! The shell, headless: GPUI's TestApp runs the real views and executors,
//! with the application's use cases over deterministic fakes.

use std::sync::Arc;

use gpui::TestApp;
use kai_application::{ActivityFeed, ContainerService, EngineService};
use kai_domain::{ContainerAction, ContainerId, ContainerState, RuntimeError};
use kai_testkit::{FakeEvents, FakeLauncher, FakeRuntime, FixedClock, container};
use kai_ui::model::{RowButton, Section};
use kai_ui::{Notice, Services, Shell, ShellOptions};

struct Harness {
    app: TestApp,
    runtime: Arc<FakeRuntime>,
    activity: Arc<ActivityFeed>,
}

fn harness() -> Harness {
    let runtime = Arc::new(FakeRuntime::with(vec![
        container("web", ContainerState::Running),
        container("old", ContainerState::Exited),
    ]));
    let activity = Arc::new(ActivityFeed::new(10));
    let mut app = TestApp::new();
    app.update(kai_ui::bind_keys);
    Harness {
        app,
        runtime,
        activity,
    }
}

fn services(h: &Harness) -> Services {
    services_with(h, None)
}

fn services_with(h: &Harness, launcher: Option<Arc<FakeLauncher>>) -> Services {
    let engine = Arc::new(EngineService::new(
        h.runtime.clone(),
        launcher.map(|l| l as Arc<dyn kai_domain::EngineLauncher>),
    ));
    let containers = Arc::new(ContainerService::new(
        h.runtime.clone(),
        Arc::new(FakeEvents::default()),
        h.activity.clone(),
        Arc::new(FixedClock::epoch()),
    ));
    Services {
        containers,
        activity: h.activity.clone(),
        engine,
    }
}

/// No polling and no event thread: the test drives every refresh.
const QUIET: ShellOptions = ShellOptions {
    poll_every: None,
    watch_engine: false,
    theme: kai_ui::ThemePreference::System,
};

fn id(s: &str) -> ContainerId {
    ContainerId::new(s).unwrap()
}

#[test]
fn opens_on_the_overview_and_lists_the_engines_containers() {
    let mut h = harness();
    let s = services(&h);
    let window = h
        .app
        .open_window(|window, cx| Shell::new(s, QUIET, window, cx));
    h.app.run_until_parked();
    window.read(|shell, _| {
        assert_eq!(shell.section(), Section::Overview);
        let names: Vec<_> = shell.containers().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["web", "old"], "active first");
        assert!(!shell.is_refreshing());
    });
}

#[test]
fn keyboard_shortcuts_switch_sections() {
    let mut h = harness();
    let s = services(&h);
    let mut window = h
        .app
        .open_window(|window, cx| Shell::new(s, QUIET, window, cx));
    window.simulate_keystrokes("secondary-2");
    window.read(|shell, _| assert_eq!(shell.section(), Section::Containers));
    window.simulate_keystrokes("secondary-1");
    window.read(|shell, _| assert_eq!(shell.section(), Section::Overview));
}

#[test]
fn stopping_a_container_reaches_the_engine_and_says_so() {
    let mut h = harness();
    let s = services(&h);
    let mut window = h
        .app
        .open_window(|window, cx| Shell::new(s, QUIET, window, cx));
    h.app.run_until_parked();
    window.update(|shell, _, cx| {
        shell.press(id("web"), RowButton::Action(ContainerAction::Stop), cx)
    });
    h.app.run_until_parked();
    assert_eq!(h.runtime.state_of("web"), Some(ContainerState::Exited));
    window.read(|shell, _| {
        assert_eq!(shell.notice(), Some(&Notice::Done("Stopped web".into())));
        assert!(
            shell
                .containers()
                .iter()
                .all(|c| c.state == ContainerState::Exited),
            "the list was refreshed"
        );
    });
    assert_eq!(
        h.activity.recent(5).len(),
        1,
        "the action reached the activity feed"
    );
}

#[test]
fn remove_takes_two_presses_and_cancel_backs_out() {
    let mut h = harness();
    let s = services(&h);
    let mut window = h
        .app
        .open_window(|window, cx| Shell::new(s, QUIET, window, cx));
    h.app.run_until_parked();

    window.update(|shell, _, cx| {
        shell.press(id("old"), RowButton::Action(ContainerAction::Remove), cx)
    });
    window.update(|shell, _, cx| shell.press(id("old"), RowButton::CancelConfirm, cx));
    h.app.run_until_parked();
    assert!(
        h.runtime.calls().is_empty(),
        "nothing reaches the engine before confirmation"
    );

    window.update(|shell, _, cx| {
        shell.press(id("old"), RowButton::Action(ContainerAction::Remove), cx)
    });
    h.app.run_until_parked();
    assert!(h.runtime.calls().is_empty());
    window.update(|shell, _, cx| {
        shell.press(id("old"), RowButton::Confirm(ContainerAction::Remove), cx)
    });
    h.app.run_until_parked();
    assert_eq!(h.runtime.state_of("old"), None);
    window.read(|shell, _| assert_eq!(shell.notice(), Some(&Notice::Done("Removed old".into()))));
}

#[test]
fn an_engine_failure_is_shown_not_swallowed() {
    let mut h = harness();
    let s = services(&h);
    let mut window = h
        .app
        .open_window(|window, cx| Shell::new(s, QUIET, window, cx));
    h.app.run_until_parked();
    h.runtime
        .fail_next(RuntimeError::Engine("port 80 is already in use".into()));
    window.update(|shell, _, cx| {
        shell.press(id("old"), RowButton::Action(ContainerAction::Start), cx)
    });
    h.app.run_until_parked();
    window.read(|shell, _| {
        assert_eq!(
            shell.notice(),
            Some(&Notice::Failed("port 80 is already in use".into()))
        )
    });
}

#[test]
fn an_engine_that_goes_away_and_comes_back_is_followed() {
    let mut h = harness();
    let s = services(&h);
    let mut window = h
        .app
        .open_window(|window, cx| Shell::new(s, QUIET, window, cx));
    h.app.run_until_parked();
    h.runtime.set_reachable(false);
    window.update(|shell, _, cx| shell.refresh(cx));
    h.app.run_until_parked();
    window.read(|shell, _| assert!(shell.containers().is_empty()));
    h.runtime.set_reachable(true);
    window.simulate_keystrokes("secondary-r");
    h.app.run_until_parked();
    window.read(|shell, _| assert_eq!(shell.containers().len(), 2));
}

#[test]
fn renders_both_sections_without_panicking() {
    let mut h = harness();
    let s = services(&h);
    let mut window = h
        .app
        .open_window(|window, cx| Shell::new(s, QUIET, window, cx));
    h.app.run_until_parked();
    window.draw();
    window.update(|shell, _, cx| {
        shell.show(Section::Containers, cx);
        shell.press(id("old"), RowButton::Action(ContainerAction::Remove), cx);
    });
    window.draw();
}

#[test]
fn opening_kai_starts_the_engine_it_ships_with() {
    let mut h = harness();
    h.runtime.set_reachable(false);
    let launcher = Arc::new(FakeLauncher::starting(h.runtime.clone()));
    let s = services_with(&h, Some(launcher.clone()));
    let window = h
        .app
        .open_window(|window, cx| Shell::new(s, QUIET, window, cx));
    h.app.run_until_parked();
    assert_eq!(launcher.launches(), 1);
    window.read(|shell, _| {
        assert!(!shell.is_starting_engine());
        assert_eq!(
            shell.notice(),
            Some(&Notice::Done("Started Kiri fake".into()))
        );
        assert_eq!(shell.containers().len(), 2, "the list came after the start");
    });
}

#[test]
fn an_engine_already_running_is_not_started_again() {
    let mut h = harness();
    let launcher = Arc::new(FakeLauncher::starting(h.runtime.clone()));
    let s = services_with(&h, Some(launcher.clone()));
    let window = h
        .app
        .open_window(|window, cx| Shell::new(s, QUIET, window, cx));
    h.app.run_until_parked();
    assert_eq!(launcher.launches(), 0);
    window.read(|shell, _| assert_eq!(shell.notice(), None));
}

#[test]
fn a_failed_start_says_why() {
    let mut h = harness();
    h.runtime.set_reachable(false);
    let s = services_with(
        &h,
        Some(Arc::new(FakeLauncher::failing("the VM did not boot"))),
    );
    let window = h
        .app
        .open_window(|window, cx| Shell::new(s, QUIET, window, cx));
    h.app.run_until_parked();
    window.read(|shell, _| {
        assert!(
            matches!(shell.notice(), Some(Notice::Failed(m)) if m.contains("the VM did not boot"))
        );
    });
}
