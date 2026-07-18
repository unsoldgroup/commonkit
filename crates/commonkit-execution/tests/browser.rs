mod support;
use commonkit_contracts::{BrowserProfile, BrowserShard};
use commonkit_execution::browser::*;

fn profile() -> BrowserProfile {
    BrowserProfile {
        chromium_revision: "123456".into(),
        operating_system: "linux-x86_64".into(),
        fonts_digest: support::digest('d'),
        locale: "en-US".into(),
        timezone: "UTC".into(),
        viewport_width: 1280,
        viewport_height: 720,
        device_scale_factor_milli: 1000,
        headless: true,
        performance_class: None,
    }
}
#[test]
fn aggregates_only_comparable_pinned_browser_shards() {
    let environment = environment_digest(&profile()).unwrap();
    let results = vec![
        BrowserShardResult {
            shard: BrowserShard {
                shard_id: "shard-1".into(),
                ordinal: 1,
                task_id: "a".into(),
            },
            environment_digest: environment.clone(),
            artifact_ids: vec!["trace-1".into()],
            passed: true,
        },
        BrowserShardResult {
            shard: BrowserShard {
                shard_id: "shard-2".into(),
                ordinal: 2,
                task_id: "b".into(),
            },
            environment_digest: environment,
            artifact_ids: vec!["trace-2".into()],
            passed: false,
        },
    ];
    let summary = aggregate(&profile(), &results).unwrap();
    assert_eq!(summary.shard_count, 2);
    assert_eq!(summary.passed, 1);
    let mut mismatch = results;
    mismatch[1].environment_digest = support::digest('e');
    assert!(matches!(
        aggregate(&profile(), &mismatch),
        Err(BrowserError::IncomparableEnvironment)
    ));
}
