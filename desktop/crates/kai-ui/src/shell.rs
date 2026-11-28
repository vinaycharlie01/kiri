//! The window's root view: sidebar, section content and status bar.
//!
//! Every use case runs on a background thread (`background_spawn`); results
//! come back to the UI thread through `Entity::update`. The engine's event
//! stream is followed on its own thread and only *triggers* refreshes, which
//! go through the ListContainers use case like any other.

use std::collections::HashSet;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::StreamExt;
use futures::channel::mpsc;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Div, ElementId, FocusHandle, FontWeight, KeyBinding, SharedString,
    Stateful, Subscription, Task, Window, actions, div, px, rgb,
};
use kai_application::{
    ActivityFeed, ContainerActionRequest, ContainerList, ContainerService, EngineService,
    EngineStart, EngineStatus, ListContainers,
};
use kai_domain::{Container, ContainerAction, ContainerId, ContainerState};

use crate::model::{self, NAV, NavItem, RowButton, Section};
use crate::theme::{Theme, ThemePreference};

actions!(kai, [Refresh, ShowOverview, ShowContainers, ToggleStopped]);

const KEY_CONTEXT: &str = "KaiShell";

/// The shell's keyboard shortcuts (`secondary` is ⌘ on macOS, Ctrl elsewhere).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-r", Refresh, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-1", ShowOverview, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-2", ShowContainers, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-a", ToggleStopped, Some(KEY_CONTEXT)),
    ]);
}

/// The use cases the shell drives. Built by a composition root.
#[derive(Clone)]
pub struct Services {
    pub containers: Arc<ContainerService>,
    pub activity: Arc<ActivityFeed>,
    pub engine: Arc<EngineService>,
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services").finish_non_exhaustive()
    }
}

/// Background behaviour, switchable so tests stay deterministic.
#[derive(Clone, Copy, Debug)]
pub struct ShellOptions {
    /// Refresh this often regardless of events (status lines like "Up 5
    /// minutes" age, and an engine that comes back is noticed).
    pub poll_every: Option<Duration>,
    /// Follow the engine's event stream for immediate refreshes.
    pub watch_engine: bool,
    /// Dark, light, or follow the operating system.
    pub theme: ThemePreference,
}

impl Default for ShellOptions {
    fn default() -> Self {
        Self {
            poll_every: Some(Duration::from_secs(5)),
            watch_engine: true,
            theme: ThemePreference::System,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    Done(String),
    Failed(String),
}

pub struct Shell {
    services: Services,
    focus: FocusHandle,
    section: Section,
    status: Option<EngineStatus>,
    list: ContainerList,
    show_stopped: bool,
    busy: HashSet<ContainerId>,
    confirming: Option<(ContainerId, ContainerAction)>,
    notice: Option<Notice>,
    refreshing: Option<Task<()>>,
    refresh_again: bool,
    /// Kai is starting the engine it ships with.
    starting_engine: bool,
    theme: ThemePreference,
    _appearance: Subscription,
    _background: Vec<Task<()>>,
}

impl std::fmt::Debug for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shell")
            .field("section", &self.section)
            .finish_non_exhaustive()
    }
}

impl Shell {
    pub fn new(
        services: Services,
        options: ShellOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Repaint at once when the OS switches between dark and light.
        let appearance = cx.observe_window_appearance(window, |_, _, cx| cx.notify());
        let mut background = Vec::new();
        if options.watch_engine {
            background.push(Self::watch(services.containers.clone(), cx));
        }
        if let Some(every) = options.poll_every {
            background.push(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(every).await;
                    if this.update(cx, |s, cx| s.refresh(cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        let mut shell = Self {
            services,
            focus,
            section: Section::Overview,
            status: None,
            list: ContainerList::default(),
            show_stopped: true,
            busy: HashSet::new(),
            confirming: None,
            notice: None,
            refreshing: None,
            refresh_again: false,
            starting_engine: false,
            theme: options.theme,
            _appearance: appearance,
            _background: background,
        };
        if shell.services.engine.can_start() {
            shell.start_engine(cx);
        } else {
            shell.refresh(cx);
        }
        shell
    }

    /// Follows engine events on a thread of its own (the port blocks), and
    /// turns bursts of them into one refresh. Reconnects every 2 s while the
    /// shell is alive; the thread ends at the next event after it is gone.
    fn watch(service: Arc<ContainerService>, cx: &mut Context<Self>) -> Task<()> {
        let (tx, mut rx) = mpsc::unbounded::<()>();
        let spawned = std::thread::Builder::new()
            .name("kai-engine-events".into())
            .spawn(move || {
                loop {
                    let _ = service.watch_changes(&mut |_| {
                        if tx.unbounded_send(()).is_ok() {
                            ControlFlow::Continue(())
                        } else {
                            ControlFlow::Break(())
                        }
                    });
                    if tx.is_closed() {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            });
        if spawned.is_err() {
            // Polling still refreshes; only the immediacy is lost.
            return Task::ready(());
        }
        cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                // Engines send events in bursts (create, start, attach…).
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
                while rx.try_recv().is_ok() {}
                if this.update(cx, |s, cx| s.refresh(cx)).is_err() {
                    break;
                }
            }
        })
    }

    /// Re-reads engine status and containers. At most one refresh runs; a
    /// request during one queues exactly one more.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing.is_some() {
            self.refresh_again = true;
            return;
        }
        let service = self.services.containers.clone();
        self.refreshing = Some(cx.spawn(async move |this, cx| {
            let (status, list) = cx
                .background_spawn(async move {
                    let status = service.engine_status();
                    let list = match status {
                        EngineStatus::Connected(_) => service
                            .list_containers(ListContainers {
                                include_stopped: true,
                            })
                            .map_err(|e| e.to_string()),
                        EngineStatus::Unreachable(_) => Ok(ContainerList::default()),
                    };
                    (status, list)
                })
                .await;
            let _ = this.update(cx, |s, cx| {
                s.refreshing = None;
                s.status = Some(status);
                match list {
                    Ok(list) => s.list = list,
                    Err(e) => s.notice = Some(Notice::Failed(e)),
                }
                if s.refresh_again {
                    s.refresh_again = false;
                    s.refresh(cx);
                }
                cx.notify();
            });
        }));
    }

    /// StartEngine: starts the engine Kai ships with if it is not running,
    /// then refreshes. Does nothing while a start is under way.
    pub fn start_engine(&mut self, cx: &mut Context<Self>) {
        if self.starting_engine {
            return;
        }
        self.starting_engine = true;
        cx.notify();
        let engine = self.services.engine.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { engine.ensure_running() })
                .await;
            let _ = this.update(cx, |s, cx| {
                s.starting_engine = false;
                match result {
                    Ok(EngineStart::Started(info)) => {
                        s.notice = Some(Notice::Done(format!(
                            "Started Kiri {}",
                            info.engine_version
                        )))
                    }
                    Ok(EngineStart::AlreadyRunning(_)) => {}
                    Ok(EngineStart::NotRunning(why)) => s.notice = Some(Notice::Failed(why)),
                    Err(e) => {
                        s.notice = Some(Notice::Failed(format!(
                            "Could not start Kiri's engine: {e}"
                        )))
                    }
                }
                s.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn is_starting_engine(&self) -> bool {
        self.starting_engine
    }

    pub fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        cx.notify();
    }

    pub fn section(&self) -> Section {
        self.section
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn containers(&self) -> &[Container] {
        &self.list.containers
    }

    pub fn is_refreshing(&self) -> bool {
        self.refreshing.is_some()
    }

    /// A row button was pressed.
    pub fn press(&mut self, id: ContainerId, button: RowButton, cx: &mut Context<Self>) {
        match button {
            RowButton::CancelConfirm => self.confirming = None,
            RowButton::Action(a) if a.is_destructive() => self.confirming = Some((id, a)),
            RowButton::Action(a) => self.perform(id, a, false, cx),
            RowButton::Confirm(a) => {
                self.confirming = None;
                self.perform(id, a, true, cx);
            }
        }
        cx.notify();
    }

    fn perform(
        &mut self,
        id: ContainerId,
        action: ContainerAction,
        confirmed: bool,
        cx: &mut Context<Self>,
    ) {
        self.busy.insert(id.clone());
        let service = self.services.containers.clone();
        cx.spawn(async move |this, cx| {
            let req = ContainerActionRequest {
                id: id.clone(),
                action,
                confirmed,
            };
            let result = cx
                .background_spawn(async move { service.perform_action(req) })
                .await;
            let _ = this.update(cx, |s, cx| {
                s.busy.remove(&id);
                s.notice = Some(match result {
                    Ok(out) => Notice::Done(model::done_notice(out.action, &out.name)),
                    Err(e) => Notice::Failed(e.to_string()),
                });
                s.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

// ---------------------------------------------------------------- rendering

fn id(parts: &[&str]) -> ElementId {
    ElementId::Name(SharedString::from(parts.join(":")))
}

fn button(theme: &Theme, key: ElementId, label: &'static str, danger: bool) -> Stateful<Div> {
    let (bg, fg) = if danger {
        (Theme::color(theme.danger), theme.danger_text)
    } else {
        (Theme::tint(theme.accent, 0), theme.text)
    };
    div()
        .id(key)
        .px_2()
        .py(px(3.))
        .rounded_md()
        .border_1()
        .border_color(rgb(if danger { theme.danger } else { theme.border }))
        .bg(bg)
        .text_color(rgb(fg))
        .text_xs()
        .cursor_pointer()
        .hover(|s| {
            s.bg(rgb(if danger {
                theme.danger
            } else {
                theme.surface_hover
            }))
        })
        .child(label)
}

fn state_colour(theme: &Theme, state: &ContainerState) -> u32 {
    match state {
        ContainerState::Running => theme.ok,
        ContainerState::Paused | ContainerState::Restarting | ContainerState::Removing => {
            theme.warn
        }
        ContainerState::Dead => theme.danger,
        _ => theme.muted,
    }
}

fn ago(at: SystemTime) -> String {
    let s = SystemTime::now()
        .duration_since(at)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match s {
        0..60 => format!("{s}s ago"),
        60..3600 => format!("{}m ago", s / 60),
        _ => format!("{}h ago", s / 3600),
    }
}

impl Shell {
    fn sidebar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut nav = div()
            .id("nav")
            .flex()
            .flex_col()
            .gap(px(1.))
            .px_2()
            .pb_3()
            .overflow_y_scroll()
            .flex_1();
        for (group, items) in NAV {
            if !group.is_empty() {
                nav = nav.child(
                    div()
                        .pt_3()
                        .pb_1()
                        .px_2()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(theme.faint))
                        .child(group.to_uppercase()),
                );
            }
            for item in *items {
                nav = nav.child(match *item {
                    NavItem::Live(section) => {
                        let active = self.section == section;
                        div()
                            .id(id(&["nav", section.title()]))
                            .px_2()
                            .py(px(5.))
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .text_color(rgb(if active { theme.text } else { theme.muted }))
                            .when(active, |d| {
                                d.bg(rgb(theme.surface_hover))
                                    .font_weight(FontWeight::MEDIUM)
                            })
                            .hover(|d| d.bg(rgb(theme.surface_hover)))
                            .on_click(cx.listener(move |s, _, _, cx| s.show(section, cx)))
                            .child(section.title())
                            .into_any_element()
                    }
                    NavItem::Planned { label, phase } => div()
                        .flex()
                        .justify_between()
                        .items_center()
                        .px_2()
                        .py(px(5.))
                        .text_sm()
                        .text_color(rgb(theme.faint))
                        .child(label)
                        .child(div().text_xs().child(format!("P{phase}")))
                        .into_any_element(),
                });
            }
        }
        div()
            .flex()
            .flex_col()
            .w(px(220.))
            .h_full()
            .bg(rgb(theme.sidebar))
            .border_r_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(theme.text))
                            .child("Kai"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.muted))
                            .child("AI agents on Kiri"),
                    ),
            )
            .child(nav)
    }

    fn header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut right = div().flex().gap_2().items_center();
        if self.refreshing.is_some() {
            right = right.child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.faint))
                    .child("Refreshing…"),
            );
        }
        if self.section == Section::Containers {
            let label = if self.show_stopped {
                "Hide stopped"
            } else {
                "Show stopped"
            };
            right = right.child(
                button(theme, "toggle-stopped".into(), label, false).on_click(cx.listener(
                    |s, _, _, cx| {
                        s.show_stopped = !s.show_stopped;
                        cx.notify();
                    },
                )),
            );
        }
        right = right.child(
            button(theme, "refresh".into(), "Refresh", false)
                .on_click(cx.listener(|s, _, _, cx| s.refresh(cx))),
        );
        div()
            .flex()
            .justify_between()
            .items_center()
            .px_6()
            .py_3()
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme.text))
                    .child(self.section.title()),
            )
            .child(right)
    }

    fn notice_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (colour, text) = match self.notice.as_ref()? {
            Notice::Done(t) => (theme.ok, t.clone()),
            Notice::Failed(t) => (theme.danger, t.clone()),
        };
        Some(
            div()
                .mx_6()
                .mt_3()
                .px_3()
                .py_2()
                .flex()
                .justify_between()
                .items_center()
                .rounded_md()
                .border_1()
                .border_color(rgb(colour))
                .bg(Theme::tint(colour, 0x1f))
                .text_sm()
                .text_color(rgb(theme.text))
                .child(div().flex_1().min_w(px(0.)).child(text))
                .child(
                    div()
                        .id("dismiss")
                        .px_1()
                        .cursor_pointer()
                        .text_color(rgb(theme.muted))
                        .child("✕")
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.notice = None;
                            cx.notify();
                        })),
                ),
        )
    }

    fn card(theme: &Theme, title: &'static str) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.surface))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme.muted))
                    .child(title.to_uppercase()),
            )
    }

    fn overview(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let can_start = self.services.engine.can_start();
        let engine = match &self.status {
            _ if self.starting_engine => Self::card(theme, "Engine")
                .child(div().text_lg().text_color(rgb(theme.text)).child("Starting Kiri's engine…"))
                .child(div().text_sm().text_color(rgb(theme.muted)).child("The first start sets Kiri up; it takes a few seconds.")),
            None => Self::card(theme, "Engine")
                .child(div().text_color(rgb(theme.muted)).child("Connecting…")),
            Some(EngineStatus::Connected(info)) => Self::card(theme, "Engine")
                .child(
                    div()
                        .text_lg()
                        .text_color(rgb(theme.text))
                        .child(format!("Kiri {}", info.engine_version)),
                )
                .child(div().text_sm().text_color(rgb(theme.muted)).child(format!(
                    "Docker API {} · {}/{}",
                    info.api_version, info.os, info.arch
                ))),
            Some(EngineStatus::Unreachable(why)) => Self::card(theme, "Engine")
                .child(
                    div()
                        .text_lg()
                        .text_color(rgb(theme.danger))
                        .child("Not reachable"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(theme.muted))
                        .child(why.clone()),
                )
                .child(div().text_sm().text_color(rgb(theme.muted)).child(if can_start {
                    "Kai includes Kiri's engine: start it here."
                } else {
                    "Start Kiri (open the app, or run `kiri daemon`). Kai reconnects by itself."
                }))
                .when(can_start, |card| {
                    card.child(
                        div().flex().child(
                            button(theme, "start-engine".into(), "Start engine", false)
                                .on_click(cx.listener(|s, _, _, cx| s.start_engine(cx))),
                        ),
                    )
                }),
        };
        let total = self.list.containers.len();
        let containers = Self::card(theme, "Containers")
            .child(
                div()
                    .text_lg()
                    .text_color(rgb(theme.text))
                    .child(format!("{} running", self.list.running)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(theme.muted))
                    .child(format!("{total} in total")),
            );
        let recent = self.services.activity.recent(8);
        let mut activity = Self::card(theme, "Recent activity");
        if recent.is_empty() {
            activity = activity.child(
                div()
                    .text_sm()
                    .text_color(rgb(theme.muted))
                    .child("Nothing yet. Actions you take in Kai show here."),
            );
        }
        for e in recent {
            activity = activity.child(
                div()
                    .flex()
                    .justify_between()
                    .text_sm()
                    .child(div().text_color(rgb(theme.text)).child(e.summary()))
                    .child(div().text_color(rgb(theme.faint)).child(ago(e.at()))),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(engine.flex_1().min_w(px(0.)))
                    .child(containers.w(px(260.))),
            )
            .child(activity)
            .into_any_element()
    }

    fn containers_view(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let visible: Vec<&Container> = self
            .list
            .containers
            .iter()
            .filter(|c| self.show_stopped || c.state.is_active())
            .collect();
        if visible.is_empty() {
            let text = match &self.status {
                Some(EngineStatus::Unreachable(_)) => {
                    "Kiri's engine is not reachable. Start Kiri, and containers appear here."
                }
                None => "Connecting to Kiri…",
                Some(EngineStatus::Connected(_)) if self.list.containers.is_empty() => {
                    "No containers yet. Run one, e.g. `kiri run -d --name web nginx`, and it appears here."
                }
                Some(EngineStatus::Connected(_)) => {
                    "No running containers. Show stopped ones with the button above."
                }
            };
            return div()
                .p_6()
                .text_sm()
                .text_color(rgb(theme.muted))
                .child(text)
                .into_any_element();
        }
        let head = |w: Option<f32>, t: &'static str| {
            let d = div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme.faint))
                .child(t.to_uppercase());
            match w {
                Some(w) => d.w(px(w)),
                None => d.flex_1(),
            }
        };
        let mut table = div()
            .id("containers")
            .flex()
            .flex_col()
            .px_6()
            .py_3()
            .overflow_y_scroll()
            .flex_1()
            .child(
                div()
                    .flex()
                    .gap_3()
                    .px_3()
                    .pb_2()
                    .child(head(None, "Name"))
                    .child(head(Some(190.), "Image"))
                    .child(head(Some(90.), "State"))
                    .child(head(Some(150.), "Ports"))
                    .child(head(Some(250.), "Actions")),
            );
        for c in visible {
            let busy = self.busy.contains(&c.id);
            let confirming = self
                .confirming
                .as_ref()
                .filter(|(cid, _)| cid == &c.id)
                .map(|(_, a)| *a);
            let mut actions = div().w(px(250.)).flex().gap_1();
            if busy {
                actions = actions.child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.muted))
                        .child("Working…"),
                );
            }
            for b in model::row_buttons(c, busy, confirming) {
                let cid = c.id.clone();
                actions = actions.child(
                    button(
                        theme,
                        id(&["row", c.id.as_str(), b.label()]),
                        b.label(),
                        b.is_danger(),
                    )
                    .on_click(cx.listener(move |s, _, _, cx| s.press(cid.clone(), b, cx))),
                );
            }
            let ports = c
                .ports
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let colour = state_colour(theme, &c.state);
            table = table.child(
                div()
                    .flex()
                    .gap_3()
                    .items_center()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(theme.border))
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(120.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_color(rgb(theme.text))
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .child(c.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.faint))
                                    .truncate()
                                    .child(match &c.compose_project {
                                        Some(p) => format!("{} · {}", c.id.short(), p),
                                        None => c.id.short().to_owned(),
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .w(px(190.))
                            .text_color(rgb(theme.muted))
                            .truncate()
                            .child(c.image.clone()),
                    )
                    .child(
                        div()
                            .w(px(90.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().size(px(7.)).rounded_full().bg(rgb(colour)))
                                    .child(
                                        div()
                                            .text_color(rgb(colour))
                                            .child(c.state.label().to_owned()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.faint))
                                    .truncate()
                                    .child(c.status.clone()),
                            ),
                    )
                    .child(
                        div()
                            .w(px(150.))
                            .text_xs()
                            .text_color(rgb(theme.muted))
                            .truncate()
                            .child(ports),
                    )
                    .child(actions),
            );
        }
        table.into_any_element()
    }

    fn status_bar(&self, theme: &Theme) -> impl IntoElement {
        let (colour, text) = match &self.status {
            _ if self.starting_engine => (theme.warn, "Starting Kiri's engine…".to_owned()),
            None => (theme.faint, "Connecting to Kiri…".to_owned()),
            Some(EngineStatus::Connected(i)) => (
                theme.ok,
                format!(
                    "Kiri {} · {} containers, {} running",
                    i.engine_version,
                    self.list.containers.len(),
                    self.list.running
                ),
            ),
            Some(EngineStatus::Unreachable(_)) => {
                (theme.danger, "Kiri's engine is not reachable".to_owned())
            }
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_1()
            .border_t_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.sidebar))
            .text_xs()
            .text_color(rgb(theme.muted))
            .child(div().size(px(7.)).rounded_full().bg(rgb(colour)))
            .child(text)
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.resolve(window.appearance());
        let content = match self.section {
            Section::Overview => self.overview(&theme, cx),
            Section::Containers => self.containers_view(&theme, cx),
        };
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|s, _: &Refresh, _, cx| s.refresh(cx)))
            .on_action(cx.listener(|s, _: &ShowOverview, _, cx| s.show(Section::Overview, cx)))
            .on_action(cx.listener(|s, _: &ShowContainers, _, cx| s.show(Section::Containers, cx)))
            .on_action(cx.listener(|s, _: &ToggleStopped, _, cx| {
                s.show_stopped = !s.show_stopped;
                cx.notify();
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.bg))
            .text_color(rgb(theme.text))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.sidebar(&theme, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .child(self.header(&theme, cx))
                            .children(self.notice_bar(&theme, cx))
                            .child(content),
                    ),
            )
            .child(self.status_bar(&theme))
    }
}
