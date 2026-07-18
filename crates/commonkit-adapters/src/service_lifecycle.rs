use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use commonkit_contracts::{
    Operation, OperationKind, ResourceRef, Risk, Sha256Digest, StableId, digest_domain_json,
};
use commonkit_core::{OperationDraft, finalize_operation};
use commonkit_reconcile::{Adapter, AdapterFailure};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStartMode {
    Automatic,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceSpec {
    pub name: String,
    pub executable: String,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub start_mode: ServiceStartMode,
}

impl ServiceSpec {
    pub fn digest(&self) -> Result<Sha256Digest, commonkit_contracts::ContractError> {
        digest_domain_json("commonkit.service-definition.v1", self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceDesiredState {
    Absent,
    Stopped,
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceObservedState {
    pub installed: bool,
    pub running: bool,
    pub definition_digest: Option<Sha256Digest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleCommand {
    Install { backend: String, spec: ServiceSpec },
    Start { backend: String, name: String },
    Stop { backend: String, name: String },
    Remove { backend: String, name: String },
}

impl LifecycleCommand {
    pub fn install_argv(
        backend: &str,
        spec: &ServiceSpec,
    ) -> Result<Vec<&'static str>, ServiceError> {
        validate_name(&spec.name)?;
        match backend {
            "launchd" => Ok(vec![
                "launchctl",
                "bootstrap",
                "gui/current",
                "<definition>",
            ]),
            "systemd_user" => Ok(vec!["systemctl", "--user", "enable", "<definition>"]),
            "windows_task" => Ok(vec![
                "schtasks.exe",
                "/Create",
                "/XML",
                "<definition>",
                "/F",
            ]),
            _ => Err(ServiceError::UnsupportedBackend),
        }
    }

    pub fn start_argv(backend: &str, name: &str) -> Result<Vec<String>, ServiceError> {
        validate_name(name)?;
        match backend {
            "launchd" => Ok(vec![
                "launchctl".into(),
                "kickstart".into(),
                format!("gui/current/{name}"),
            ]),
            "systemd_user" => Ok(vec![
                "systemctl".into(),
                "--user".into(),
                "start".into(),
                format!("{name}.service"),
            ]),
            "windows_task" => Ok(vec![
                "schtasks.exe".into(),
                "/Run".into(),
                "/TN".into(),
                name.into(),
            ]),
            _ => Err(ServiceError::UnsupportedBackend),
        }
    }

    pub fn remove_argv(backend: &str, name: &str) -> Result<Vec<String>, ServiceError> {
        validate_name(name)?;
        match backend {
            "launchd" => Ok(vec![
                "launchctl".into(),
                "bootout".into(),
                format!("gui/current/{name}"),
            ]),
            "systemd_user" => Ok(vec![
                "systemctl".into(),
                "--user".into(),
                "disable".into(),
                "--now".into(),
                format!("{name}.service"),
            ]),
            "windows_task" => Ok(vec![
                "schtasks.exe".into(),
                "/Delete".into(),
                "/TN".into(),
                name.into(),
                "/F".into(),
            ]),
            _ => Err(ServiceError::UnsupportedBackend),
        }
    }
}

pub trait ServiceBackend: Send + Sync + 'static {
    fn inspect(&self, name: &str) -> Result<ServiceObservedState, String>;
    fn execute(&self, command: &LifecycleCommand) -> Result<(), String>;
}

#[derive(Clone)]
struct Intent {
    spec: ServiceSpec,
    desired: ServiceDesiredState,
    before: ServiceObservedState,
}

pub struct ServiceAdapter<B> {
    id: StableId,
    backend_id: String,
    backend: Arc<B>,
    intents: HashMap<Sha256Digest, Intent>,
}

impl<B: ServiceBackend> ServiceAdapter<B> {
    pub fn new(backend_id: &str, backend: Arc<B>) -> Result<Self, ServiceError> {
        LifecycleCommand::remove_argv(backend_id, "commonkit-probe")?;
        Ok(Self {
            id: StableId::parse("service")?,
            backend_id: backend_id.into(),
            backend,
            intents: HashMap::new(),
        })
    }

    pub fn validate_spec(spec: &ServiceSpec) -> Result<(), ServiceError> {
        validate_name(&spec.name)?;
        if spec.executable.is_empty()
            || spec.executable.contains('\0')
            || spec.arguments.iter().any(|v| v.contains('\0'))
        {
            return Err(ServiceError::InvalidSpec);
        }
        Ok(())
    }

    pub fn register(
        &mut self,
        id: &str,
        spec: ServiceSpec,
        desired: ServiceDesiredState,
    ) -> Result<Operation, ServiceError> {
        Self::validate_spec(&spec)?;
        let before = self
            .backend
            .inspect(&spec.name)
            .map_err(ServiceError::Backend)?;
        let desired_observed = desired_observed(&spec, desired)?;
        if before == desired_observed {
            return Err(ServiceError::NoChange);
        }
        let before_digest = digest_domain_json("commonkit.service-state.v1", &before)?;
        let after_digest = digest_domain_json("commonkit.service-state.v1", &desired_observed)?;
        let payload_digest = digest_domain_json(
            "commonkit.service-intent.v1",
            &(&self.backend_id, &spec, desired),
        )?;
        let operation = finalize_operation(OperationDraft {
            adapter_id: self.id.clone(),
            kind: if desired == ServiceDesiredState::Absent {
                OperationKind::Delete
            } else if before.installed {
                OperationKind::Update
            } else {
                OperationKind::Create
            },
            resource: ResourceRef {
                resource_type: StableId::parse("service")?,
                resource_id: StableId::parse(id)?,
                managed_path: None,
            },
            risk: Risk::Medium,
            requires_confirmation: true,
            depends_on: vec![],
            before_digest: Some(before_digest),
            after_digest: Some(after_digest),
            payload_digest,
            summary: format!("reconcile service {}", spec.name),
        })?;
        self.intents.insert(
            operation.id.clone(),
            Intent {
                spec,
                desired,
                before,
            },
        );
        Ok(operation)
    }

    fn intent(&self, op: &Operation) -> Result<&Intent, AdapterFailure> {
        self.intents
            .get(&op.id)
            .ok_or_else(|| failure("service_intent_missing", "service intent is unavailable"))
    }

    fn execute(&self, command: LifecycleCommand) -> Result<(), AdapterFailure> {
        self.backend
            .execute(&command)
            .map_err(|_| failure("service_backend_failed", "service backend command failed"))
    }
}

impl<B: ServiceBackend> Adapter for ServiceAdapter<B> {
    fn id(&self) -> &StableId {
        &self.id
    }
    fn prepare(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.intent(operation)?;
        let actual = self
            .backend
            .inspect(&intent.spec.name)
            .map_err(|_| failure("service_inspect_failed", "service inspection failed"))?;
        if actual == intent.before {
            Ok(())
        } else {
            Err(failure(
                "service_preimage_changed",
                "service changed after planning",
            ))
        }
    }
    fn apply(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.intent(operation)?.clone();
        if intent.desired == ServiceDesiredState::Absent {
            if intent.before.running {
                self.execute(LifecycleCommand::Stop {
                    backend: self.backend_id.clone(),
                    name: intent.spec.name.clone(),
                })?;
            }
            if intent.before.installed {
                self.execute(LifecycleCommand::Remove {
                    backend: self.backend_id.clone(),
                    name: intent.spec.name,
                })?;
            }
            return Ok(());
        }
        let desired_digest = intent
            .spec
            .digest()
            .map_err(|_| failure("service_digest_failed", "definition digest failed"))?;
        if !intent.before.installed || intent.before.definition_digest != Some(desired_digest) {
            self.execute(LifecycleCommand::Install {
                backend: self.backend_id.clone(),
                spec: intent.spec.clone(),
            })?;
        }
        let running = if intent.desired == ServiceDesiredState::Stopped {
            Some(
                self.backend
                    .inspect(&intent.spec.name)
                    .map_err(|_| failure("service_inspect_failed", "service inspection failed"))?
                    .running,
            )
        } else {
            None
        };
        match intent.desired {
            ServiceDesiredState::Running => self.execute(LifecycleCommand::Start {
                backend: self.backend_id.clone(),
                name: intent.spec.name,
            }),
            ServiceDesiredState::Stopped if running == Some(true) => {
                self.execute(LifecycleCommand::Stop {
                    backend: self.backend_id.clone(),
                    name: intent.spec.name,
                })
            }
            _ => Ok(()),
        }
    }
    fn verify(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.intent(operation)?;
        let actual = self
            .backend
            .inspect(&intent.spec.name)
            .map_err(|_| failure("service_inspect_failed", "service inspection failed"))?;
        if actual
            == desired_observed(&intent.spec, intent.desired)
                .map_err(|_| failure("service_digest_failed", "definition digest failed"))?
        {
            Ok(())
        } else {
            Err(failure(
                "service_verify_mismatch",
                "service differs from desired state",
            ))
        }
    }
    fn rollback(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.intent(operation)?.clone();
        let current = self
            .backend
            .inspect(&intent.spec.name)
            .map_err(|_| failure("service_inspect_failed", "service inspection failed"))?;
        if !intent.before.installed {
            if current.running {
                self.execute(LifecycleCommand::Stop {
                    backend: self.backend_id.clone(),
                    name: intent.spec.name.clone(),
                })?;
            }
            if current.installed {
                self.execute(LifecycleCommand::Remove {
                    backend: self.backend_id.clone(),
                    name: intent.spec.name,
                })?;
            }
        } else {
            self.execute(LifecycleCommand::Install {
                backend: self.backend_id.clone(),
                spec: intent.spec.clone(),
            })?;
            let command = if intent.before.running {
                LifecycleCommand::Start {
                    backend: self.backend_id.clone(),
                    name: intent.spec.name,
                }
            } else {
                LifecycleCommand::Stop {
                    backend: self.backend_id.clone(),
                    name: intent.spec.name,
                }
            };
            self.execute(command)?;
        }
        Ok(())
    }
}

fn desired_observed(
    spec: &ServiceSpec,
    desired: ServiceDesiredState,
) -> Result<ServiceObservedState, commonkit_contracts::ContractError> {
    Ok(match desired {
        ServiceDesiredState::Absent => ServiceObservedState::default(),
        ServiceDesiredState::Stopped => ServiceObservedState {
            installed: true,
            running: false,
            definition_digest: Some(spec.digest()?),
        },
        ServiceDesiredState::Running => ServiceObservedState {
            installed: true,
            running: true,
            definition_digest: Some(spec.digest()?),
        },
    })
}

fn validate_name(name: &str) -> Result<(), ServiceError> {
    if !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        Ok(())
    } else {
        Err(ServiceError::InvalidName)
    }
}

fn failure(code: &str, message: &str) -> AdapterFailure {
    AdapterFailure {
        code: StableId::parse(code).expect("static ID").to_string(),
        message: message.into(),
    }
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("invalid service name")]
    InvalidName,
    #[error("invalid service specification")]
    InvalidSpec,
    #[error("unsupported service backend")]
    UnsupportedBackend,
    #[error("service already matches desired state")]
    NoChange,
    #[error("service backend failed: {0}")]
    Backend(String),
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::ContractError),
    #[error(transparent)]
    Plan(#[from] commonkit_core::PlanBuildError),
}
