mod support;
use commonkit_contracts::StableId;
use commonkit_execution::implementor::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;

struct ScriptedOrca {
    references: Mutex<BTreeMap<String, String>>,
}
impl OrcaTransport for ScriptedOrca {
    fn call(&self, operation: &str, payload: Value) -> Result<Value, ImplementorError> {
        Ok(match operation {
            "start" => {
                let key = payload["idempotencyKey"].as_str().unwrap();
                let mut refs = self.references.lock().unwrap();
                let reference = refs
                    .entry(key.into())
                    .or_insert_with(|| "opaque-orca-run".into())
                    .clone();
                json!({"engineRunRef":reference})
            }
            "events" => json!([{"cursor":1,"kind":"started","data":{}}]),
            "result" => json!({"state":"running","engineRunRef":"ignored","output":{}}),
            _ => json!({"ok":true}),
        })
    }
}

fn conformance(adapter: &dyn ImplementorAdapter) {
    let manifest = support::manifest();
    let first = adapter.start(&manifest, "same-delivery").unwrap();
    let duplicate = adapter.start(&manifest, "same-delivery").unwrap();
    assert_eq!(
        first, duplicate,
        "duplicate delivery must not duplicate runs"
    );
    assert!(!first.is_empty());
    assert_eq!(adapter.events(&first, 0).unwrap()[0].cursor, 1);
    adapter.send(&first, "continue").unwrap();
    adapter.cancel(&first).unwrap();
    adapter.resume(&first, "checkpoint-1").unwrap();
    let result = adapter.result(&first).unwrap();
    assert_eq!(result.engine_run_ref, first)
}

#[test]
fn fake_adapter_passes_conformance() {
    conformance(&FakeImplementor::default())
}
#[test]
fn orca_claude_adapter_passes_conformance() {
    conformance(&OrcaImplementor::new(
        ScriptedOrca {
            references: Mutex::new(BTreeMap::new()),
        },
        StableId::parse("claude").unwrap(),
    ))
}
#[test]
fn orca_codex_adapter_passes_conformance() {
    conformance(&OrcaImplementor::new(
        ScriptedOrca {
            references: Mutex::new(BTreeMap::new()),
        },
        StableId::parse("codex").unwrap(),
    ))
}
