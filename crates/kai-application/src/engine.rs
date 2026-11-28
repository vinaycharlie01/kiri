//! StartEngine: makes sure the container engine runs, starting the one Kai
//! ships with when it does not. In the one-app install this is what lets
//! someone open Kai without installing or starting anything else.

use std::sync::Arc;

use kai_domain::{ContainerRuntime, EngineInfo, EngineLauncher};

use crate::AppError;

/// Output of StartEngine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineStart {
    /// It was running already; nothing was started.
    AlreadyRunning(EngineInfo),
    /// It was not running, and Kai started it.
    Started(EngineInfo),
    /// It is not running, and there is no engine to start (Kai was not
    /// installed with one). The text says how to start one.
    NotRunning(String),
}

pub struct EngineService {
    runtime: Arc<dyn ContainerRuntime>,
    launcher: Option<Arc<dyn EngineLauncher>>,
}

impl std::fmt::Debug for EngineService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineService")
            .field("can_start", &self.launcher.is_some())
            .finish_non_exhaustive()
    }
}

impl EngineService {
    pub fn new(
        runtime: Arc<dyn ContainerRuntime>,
        launcher: Option<Arc<dyn EngineLauncher>>,
    ) -> Self {
        Self { runtime, launcher }
    }

    /// Whether Kai can start an engine itself.
    pub fn can_start(&self) -> bool {
        self.launcher.is_some()
    }

    /// StartEngine. Blocks until the engine answers or starting it failed.
    pub fn ensure_running(&self) -> Result<EngineStart, AppError> {
        if let Ok(info) = self.runtime.engine_info() {
            return Ok(EngineStart::AlreadyRunning(info));
        }
        let Some(launcher) = &self.launcher else {
            return Ok(EngineStart::NotRunning(
                "Kiri's engine is not running, and this Kai has no engine of its own to start. \
                 Start Kiri, or install the Kai app, which includes it."
                    .into(),
            ));
        };
        launcher.launch()?;
        Ok(EngineStart::Started(self.runtime.engine_info()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kai_domain::RuntimeError;
    use kai_testkit::{FakeLauncher, FakeRuntime};

    fn service(rt: &Arc<FakeRuntime>, launcher: Option<Arc<FakeLauncher>>) -> EngineService {
        EngineService::new(rt.clone(), launcher.map(|l| l as Arc<dyn EngineLauncher>))
    }

    #[test]
    fn a_running_engine_is_left_alone() {
        let rt = Arc::new(FakeRuntime::default());
        let launcher = Arc::new(FakeLauncher::starting(rt.clone()));
        let out = service(&rt, Some(launcher.clone()))
            .ensure_running()
            .unwrap();
        assert!(matches!(out, EngineStart::AlreadyRunning(_)));
        assert_eq!(launcher.launches(), 0);
    }

    #[test]
    fn a_stopped_engine_is_started_once() {
        let rt = Arc::new(FakeRuntime::default());
        rt.set_reachable(false);
        let launcher = Arc::new(FakeLauncher::starting(rt.clone()));
        let out = service(&rt, Some(launcher.clone()))
            .ensure_running()
            .unwrap();
        assert!(matches!(out, EngineStart::Started(_)));
        assert_eq!(launcher.launches(), 1);
    }

    #[test]
    fn without_a_launcher_it_says_how_to_start_one() {
        let rt = Arc::new(FakeRuntime::default());
        rt.set_reachable(false);
        let s = service(&rt, None);
        assert!(!s.can_start());
        assert!(
            matches!(s.ensure_running().unwrap(), EngineStart::NotRunning(m) if m.contains("Start Kiri"))
        );
    }

    #[test]
    fn a_failed_start_is_an_error() {
        let rt = Arc::new(FakeRuntime::default());
        rt.set_reachable(false);
        let launcher = Arc::new(FakeLauncher::failing("the VM did not boot"));
        let err = service(&rt, Some(launcher)).ensure_running().unwrap_err();
        assert_eq!(
            err,
            AppError::Runtime(RuntimeError::Engine("the VM did not boot".into()))
        );
    }
}
