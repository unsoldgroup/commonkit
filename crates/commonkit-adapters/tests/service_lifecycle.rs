use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use commonkit_adapters::{
    LifecycleCommand, ServiceAdapter, ServiceBackend, ServiceDesiredState, ServiceObservedState,
    ServiceSpec, ServiceStartMode,
};
use commonkit_reconcile::Adapter;

#[derive(Default)]
struct FakeBackend {
    states: Mutex<BTreeMap<String, ServiceObservedState>>,
    commands: Mutex<Vec<LifecycleCommand>>,
}

impl ServiceBackend for FakeBackend {
    fn inspect(&self, name: &str) -> Result<ServiceObservedState, String> {
        Ok(self
            .states
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_default())
    }

    fn execute(&self, command: &LifecycleCommand) -> Result<(), String> {
        self.commands.lock().unwrap().push(command.clone());
        let mut states = self.states.lock().unwrap();
        match command {
            LifecycleCommand::Install { spec, .. } => {
                states.insert(
                    spec.name.clone(),
                    ServiceObservedState {
                        installed: true,
                        running: false,
                        definition_digest: Some(spec.digest().unwrap()),
                    },
                );
            }
            LifecycleCommand::Start { name, .. } => states.get_mut(name).unwrap().running = true,
            LifecycleCommand::Stop { name, .. } => states.get_mut(name).unwrap().running = false,
            LifecycleCommand::Remove { name, .. } => {
                states.remove(name);
            }
        }
        Ok(())
    }
}

fn spec() -> ServiceSpec {
    ServiceSpec {
        name: "commonkit-relay".into(),
        executable: "/opt/commonkit/bin/commonkit-relay".into(),
        arguments: vec![
            "serve".into(),
            "--config".into(),
            "/state/relay.json".into(),
        ],
        environment: BTreeMap::new(),
        start_mode: ServiceStartMode::Automatic,
    }
}

#[test]
fn lifecycle_adapter_applies_verifies_and_rolls_back_typed_state() {
    let backend = Arc::new(FakeBackend::default());
    let mut adapter = ServiceAdapter::new("systemd_user", backend.clone()).unwrap();
    let operation = adapter
        .register("relay", spec(), ServiceDesiredState::Running)
        .unwrap();

    adapter.prepare(&operation).unwrap();
    adapter.apply(&operation).unwrap();
    adapter.verify(&operation).unwrap();
    assert_eq!(backend.inspect("commonkit-relay").unwrap().running, true);

    adapter.rollback(&operation).unwrap();
    assert_eq!(
        backend.inspect("commonkit-relay").unwrap(),
        ServiceObservedState::default()
    );
}

#[test]
fn platform_commands_are_fixed_argv_without_a_shell() {
    let service = spec();
    assert_eq!(
        LifecycleCommand::install_argv("launchd", &service).unwrap(),
        vec!["launchctl", "bootstrap", "gui/current", "<definition>"]
    );
    assert_eq!(
        LifecycleCommand::start_argv("systemd_user", &service.name).unwrap(),
        vec!["systemctl", "--user", "start", "commonkit-relay.service"]
    );
    assert_eq!(
        LifecycleCommand::remove_argv("windows_task", &service.name).unwrap(),
        vec!["schtasks.exe", "/Delete", "/TN", "commonkit-relay", "/F"]
    );
}

#[test]
fn rejects_service_names_that_could_change_backend_argv_shape() {
    let mut invalid = spec();
    invalid.name = "relay; shutdown".into();
    assert!(ServiceAdapter::<FakeBackend>::validate_spec(&invalid).is_err());
}

struct FailingReinspection {
    inspections: std::sync::atomic::AtomicUsize,
}

impl ServiceBackend for FailingReinspection {
    fn inspect(&self, _name: &str) -> Result<ServiceObservedState, String> {
        if self
            .inspections
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            Ok(ServiceObservedState::default())
        } else {
            Err("platform lifecycle unavailable".into())
        }
    }
    fn execute(&self, _command: &LifecycleCommand) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn stopped_service_apply_fails_closed_when_platform_reinspection_fails() {
    let backend = Arc::new(FailingReinspection {
        inspections: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut adapter = ServiceAdapter::new("windows_task", backend).unwrap();
    let operation = adapter
        .register("relay", spec(), ServiceDesiredState::Stopped)
        .unwrap();
    let error = adapter.apply(&operation).unwrap_err();
    assert_eq!(error.code.as_str(), "service_inspect_failed");
}
