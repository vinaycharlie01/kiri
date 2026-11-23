//! `kai`: the Kai desktop application.
//!
//! This is the composition root: the one place that knows concrete adapters.
//! It builds them, hands them to the use cases as ports, and opens the native
//! window. Swapping the engine (or any other adapter) is a change here only.
//!
//!   kai [--socket PATH] [--theme system|dark|light]
//!
//! Without `--socket`, Kai finds Kiri's engine the way Kiri's own tools do:
//! `KIRI_SOCKET`, the Linux service's socket, or `~/.kiri/run/docker.sock`.

use std::sync::Arc;
use std::time::SystemTime;

use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use kai_adapter_kiri::KiriEngine;
use kai_application::{ActivityFeed, ContainerService};
use kai_domain::Clock;
use kai_ui::{Services, Shell, ShellOptions, ThemePreference};

/// The product's name, in one place: the fork's own brand, kept separate
/// from upstream Kiri's (see docs/ai-platform/IMPLEMENTATION_ROADMAP.md).
const PRODUCT: &str = "Kai";

/// The system clock, as the `Clock` port.
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

const USAGE: &str = "usage: kai [--socket PATH] [--theme system|dark|light]";

/// What the command line asks for.
struct Args {
    engine: KiriEngine,
    theme: ThemePreference,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let (mut socket, mut theme) = (None, ThemePreference::System);
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it
            .next()
            .ok_or_else(|| format!("{USAGE}\n({flag} needs a value)"))?;
        match flag.as_str() {
            "--socket" => socket = Some(value.clone()),
            "--theme" => {
                theme = ThemePreference::parse(value)
                    .ok_or_else(|| format!("{USAGE}\n(unknown theme: {value})"))?
            }
            _ => return Err(format!("{USAGE}\n(unknown option: {flag})")),
        }
    }
    let engine = match socket {
        Some(path) => KiriEngine::at(path),
        None => KiriEngine::discover().ok_or_else(|| {
            "cannot find Kiri's engine: HOME is not set; pass --socket PATH".to_owned()
        })?,
    };
    Ok(Args { engine, theme })
}

fn services(engine: KiriEngine) -> Services {
    let engine = Arc::new(engine);
    let activity = Arc::new(ActivityFeed::new(200));
    let containers = Arc::new(ContainerService::new(
        engine.clone(),
        engine,
        activity.clone(),
        Arc::new(SystemClock),
    ));
    Services {
        containers,
        activity,
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Args { engine, theme } = match parse_args(&args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("kai: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let services = services(engine);
    gpui_platform::application().run(move |cx: &mut App| {
        kai_ui::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(PRODUCT.into()),
                ..Default::default()
            }),
            app_id: Some("dev.kai.desktop".into()),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                Shell::new(
                    services,
                    ShellOptions {
                        theme,
                        ..ShellOptions::default()
                    },
                    window,
                    cx,
                )
            })
        });
        if let Err(e) = opened {
            eprintln!("kai: cannot open a window: {e}");
            cx.quit();
        }
        cx.activate(true);
    });
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Result<Args, String> {
        parse_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn socket_and_theme_can_be_given() {
        let a = args(&["--theme", "dark", "--socket", "/tmp/k.sock"]).unwrap();
        assert_eq!(a.engine.socket(), std::path::Path::new("/tmp/k.sock"));
        assert_eq!(a.theme, ThemePreference::Dark);
    }

    #[test]
    fn bad_arguments_print_usage() {
        for bad in [&["--nope", "x"][..], &["--theme", "blue"], &["--socket"]] {
            assert!(args(bad).err().unwrap().starts_with("usage:"), "{bad:?}");
        }
    }
}
