use commonkit_contracts::{ExecutionManifest, StableId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImplementorEvent {
    pub cursor: u64,
    pub kind: String,
    pub data: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImplementorResult {
    pub state: String,
    pub engine_run_ref: String,
    pub output: Value,
}

pub trait ImplementorAdapter: Send + Sync {
    fn id(&self) -> StableId;
    fn capabilities(&self) -> BTreeSet<StableId>;
    fn start(
        &self,
        manifest: &ExecutionManifest,
        idempotency_key: &str,
    ) -> Result<String, ImplementorError>;
    fn events(
        &self,
        engine_run_ref: &str,
        after: u64,
    ) -> Result<Vec<ImplementorEvent>, ImplementorError>;
    fn send(&self, engine_run_ref: &str, message: &str) -> Result<(), ImplementorError>;
    fn cancel(&self, engine_run_ref: &str) -> Result<(), ImplementorError>;
    fn resume(&self, engine_run_ref: &str, checkpoint: &str) -> Result<(), ImplementorError>;
    fn result(&self, engine_run_ref: &str) -> Result<ImplementorResult, ImplementorError>;
}

#[derive(Default)]
pub struct FakeImplementor {
    runs: Mutex<BTreeMap<String, FakeRun>>,
    idempotency: Mutex<BTreeMap<String, String>>,
}
#[derive(Clone)]
struct FakeRun {
    events: Vec<ImplementorEvent>,
    state: String,
}
impl ImplementorAdapter for FakeImplementor {
    fn id(&self) -> StableId {
        StableId::parse("fake").expect("stable")
    }
    fn capabilities(&self) -> BTreeSet<StableId> {
        BTreeSet::from([StableId::parse("fake").unwrap()])
    }
    fn start(&self, _: &ExecutionManifest, key: &str) -> Result<String, ImplementorError> {
        if let Some(reference) = self.idempotency.lock().unwrap().get(key) {
            return Ok(reference.clone());
        }
        let reference = format!("fake_{}", Uuid::new_v4().simple());
        self.runs.lock().unwrap().insert(
            reference.clone(),
            FakeRun {
                events: vec![ImplementorEvent {
                    cursor: 1,
                    kind: "started".into(),
                    data: json!({}),
                }],
                state: "running".into(),
            },
        );
        self.idempotency
            .lock()
            .unwrap()
            .insert(key.into(), reference.clone());
        Ok(reference)
    }
    fn events(
        &self,
        reference: &str,
        after: u64,
    ) -> Result<Vec<ImplementorEvent>, ImplementorError> {
        Ok(self
            .run(reference)?
            .events
            .into_iter()
            .filter(|event| event.cursor > after)
            .collect())
    }
    fn send(&self, reference: &str, message: &str) -> Result<(), ImplementorError> {
        let mut runs = self.runs.lock().unwrap();
        let run = runs.get_mut(reference).ok_or(ImplementorError::NotFound)?;
        run.events.push(ImplementorEvent {
            cursor: run.events.len() as u64 + 1,
            kind: "message".into(),
            data: json!({"length":message.len()}),
        });
        Ok(())
    }
    fn cancel(&self, reference: &str) -> Result<(), ImplementorError> {
        self.set_state(reference, "canceled")
    }
    fn resume(&self, reference: &str, _: &str) -> Result<(), ImplementorError> {
        self.set_state(reference, "running")
    }
    fn result(&self, reference: &str) -> Result<ImplementorResult, ImplementorError> {
        let run = self.run(reference)?;
        Ok(ImplementorResult {
            state: run.state,
            engine_run_ref: reference.into(),
            output: json!({}),
        })
    }
}
impl FakeImplementor {
    fn run(&self, reference: &str) -> Result<FakeRun, ImplementorError> {
        self.runs
            .lock()
            .unwrap()
            .get(reference)
            .cloned()
            .ok_or(ImplementorError::NotFound)
    }
    fn set_state(&self, reference: &str, state: &str) -> Result<(), ImplementorError> {
        let mut runs = self.runs.lock().unwrap();
        let run = runs.get_mut(reference).ok_or(ImplementorError::NotFound)?;
        run.state = state.into();
        run.events.push(ImplementorEvent {
            cursor: run.events.len() as u64 + 1,
            kind: state.into(),
            data: json!({}),
        });
        Ok(())
    }
}

pub trait OrcaTransport: Send + Sync {
    fn call(&self, operation: &str, payload: Value) -> Result<Value, ImplementorError>;
}
pub struct OrcaImplementor<T> {
    transport: T,
    engine: StableId,
}
impl<T> OrcaImplementor<T> {
    pub fn new(transport: T, engine: StableId) -> Self {
        Self { transport, engine }
    }
}
impl<T: OrcaTransport> ImplementorAdapter for OrcaImplementor<T> {
    fn id(&self) -> StableId {
        StableId::parse("orca").unwrap()
    }
    fn capabilities(&self) -> BTreeSet<StableId> {
        BTreeSet::from([self.engine.clone()])
    }
    fn start(&self, manifest: &ExecutionManifest, key: &str) -> Result<String, ImplementorError> {
        let value = self.transport.call(
            "start",
            json!({"manifest":manifest,"engine":self.engine,"idempotencyKey":key}),
        )?;
        opaque_ref(&value)
    }
    fn events(
        &self,
        reference: &str,
        after: u64,
    ) -> Result<Vec<ImplementorEvent>, ImplementorError> {
        Ok(serde_json::from_value(self.transport.call(
            "events",
            json!({"engineRunRef":reference,"after":after}),
        )?)?)
    }
    fn send(&self, reference: &str, message: &str) -> Result<(), ImplementorError> {
        self.transport
            .call("send", json!({"engineRunRef":reference,"message":message}))
            .map(|_| ())
    }
    fn cancel(&self, reference: &str) -> Result<(), ImplementorError> {
        self.transport
            .call("cancel", json!({"engineRunRef":reference}))
            .map(|_| ())
    }
    fn resume(&self, reference: &str, checkpoint: &str) -> Result<(), ImplementorError> {
        self.transport
            .call(
                "resume",
                json!({"engineRunRef":reference,"checkpoint":checkpoint}),
            )
            .map(|_| ())
    }
    fn result(&self, reference: &str) -> Result<ImplementorResult, ImplementorError> {
        let mut result: ImplementorResult = serde_json::from_value(
            self.transport
                .call("result", json!({"engineRunRef":reference}))?,
        )?;
        result.engine_run_ref = reference.into();
        Ok(result)
    }
}
fn opaque_ref(value: &Value) -> Result<String, ImplementorError> {
    value
        .get("engineRunRef")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() < 1024)
        .map(str::to_string)
        .ok_or(ImplementorError::InvalidResponse)
}
#[derive(Debug, Error)]
pub enum ImplementorError {
    #[error("implementor run not found")]
    NotFound,
    #[error("invalid implementor response")]
    InvalidResponse,
    #[error("implementor transport failed")]
    Transport,
    #[error("invalid implementor JSON")]
    Json(#[from] serde_json::Error),
}
