//! Starting the Kiri engine Kai ships with: `kiri daemon`, from the `kiri`
//! binary inside Kai.app (or wherever the composition root found it).
//!
//! As Kiri's own window does when it has no LaunchAgent
//! (`crates/kiri/src/update.rs`, `spawn_daemon`): the daemon runs in its own
//! process group, so quitting Kai or closing its terminal does not take it
//! down, and its output goes to `daemon.log` beside the socket's folder.
//! Kai only starts it; it never stops it.

use std::fs::OpenOptions;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use kai_domain::{EngineLauncher, RuntimeError};

use crate::http;

/// How long the daemon may take to answer. It answers within milliseconds of
/// starting; the VM itself boots later, on first use.
const READY_WITHIN: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
pub struct KiriLauncher {
    binary: PathBuf,
    socket: PathBuf,
}

impl KiriLauncher {
    /// `binary` is a `kiri` executable; the daemon will serve `socket`.
    pub fn new(binary: impl Into<PathBuf>, socket: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            socket: socket.into(),
        }
    }

    /// `~/.kiri/run/docker.sock` → `~/.kiri/daemon.log`, as Kiri does.
    pub fn log_path(&self) -> PathBuf {
        match self.socket.parent().and_then(Path::parent) {
            Some(state) => state.join("daemon.log"),
            None => PathBuf::from("daemon.log"),
        }
    }

    fn answers(&self) -> bool {
        http::request(&self.socket, "GET", "/_ping").is_ok_and(|r| r.status == 200)
    }
}

impl EngineLauncher for KiriLauncher {
    fn launch(&self) -> Result<(), RuntimeError> {
        if self.answers() {
            return Ok(());
        }
        if !self.binary.is_file() {
            return Err(RuntimeError::Unreachable(format!(
                "no engine to start at {}",
                self.binary.display()
            )));
        }
        let log = self.log_path();
        if let Some(dir) = log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let out = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .map_err(|e| RuntimeError::Engine(format!("cannot open {}: {e}", log.display())))?;
        let err = out
            .try_clone()
            .map_err(|e| RuntimeError::Engine(format!("cannot open {}: {e}", log.display())))?;
        let mut child = Command::new(&self.binary)
            .arg("daemon")
            .env("KIRI_SOCKET", &self.socket)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .process_group(0)
            .spawn()
            .map_err(|e| {
                RuntimeError::Engine(format!("cannot start {}: {e}", self.binary.display()))
            })?;
        let started = Instant::now();
        loop {
            if self.answers() {
                // Reaped in the background, so it never lingers as a zombie
                // while Kai runs; after Kai exits, init adopts it.
                std::thread::spawn(move || child.wait());
                return Ok(());
            }
            if let Ok(Some(status)) = child.try_wait() {
                // "engine already running" exits 0: someone else's daemon
                // won the socket a moment ago, which is fine if it answers.
                if self.answers() {
                    return Ok(());
                }
                return Err(RuntimeError::Engine(format!(
                    "Kiri's engine stopped as it started ({status}); see {}",
                    log.display()
                )));
            }
            if started.elapsed() > READY_WITHIN {
                return Err(RuntimeError::Engine(format!(
                    "Kiri's engine did not answer within {} s; see {}",
                    READY_WITHIN.as_secs(),
                    log.display()
                )));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn describe(&self) -> String {
        format!("Kiri at {}", self.binary.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kai-launch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("run")).unwrap();
        d
    }

    fn script(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("kiri");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn the_log_sits_beside_the_run_folder_as_kiris_does() {
        let l = KiriLauncher::new("/x/kiri", "/home/me/.kiri/run/docker.sock");
        assert_eq!(l.log_path(), Path::new("/home/me/.kiri/daemon.log"));
    }

    #[test]
    fn a_missing_binary_is_reported() {
        let d = temp("missing");
        let l = KiriLauncher::new(d.join("nope"), d.join("run/docker.sock"));
        assert!(matches!(l.launch(), Err(RuntimeError::Unreachable(m)) if m.contains("no engine")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_engine_that_exits_at_once_points_at_its_log() {
        let d = temp("dies");
        let bin = script(&d, "echo 'kiri: no VM for you' >&2; exit 3");
        let l = KiriLauncher::new(bin, d.join("run/docker.sock"));
        let err = l.launch().unwrap_err().to_string();
        assert!(
            err.contains("stopped as it started") && err.contains("daemon.log"),
            "{err}"
        );
        let log = std::fs::read_to_string(l.log_path()).unwrap();
        assert!(
            log.contains("no VM for you"),
            "the daemon's output went to its log"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_daemon_is_told_which_socket_to_serve() {
        let d = temp("socket");
        let bin = script(&d, "echo \"$1 $KIRI_SOCKET\"; exit 1");
        let sock = d.join("run/docker.sock");
        let l = KiriLauncher::new(bin, &sock);
        let _ = l.launch();
        let log = std::fs::read_to_string(l.log_path()).unwrap();
        assert_eq!(log.trim(), format!("daemon {}", sock.display()));
        let _ = std::fs::remove_dir_all(&d);
    }
}
