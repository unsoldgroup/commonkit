use commonkit_contracts::{
    BrowserProfile, BrowserShard, Checkpoint, Sha256Digest, digest_domain_json,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserShardResult {
    pub shard: BrowserShard,
    pub environment_digest: Sha256Digest,
    pub artifact_ids: Vec<String>,
    pub passed: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserAggregate {
    pub environment_digest: Sha256Digest,
    pub result_digest: Sha256Digest,
    pub shard_count: u32,
    pub passed: u32,
    pub failed: u32,
    pub artifact_ids: Vec<String>,
}

pub fn environment_digest(profile: &BrowserProfile) -> Result<Sha256Digest, BrowserError> {
    Ok(digest_domain_json(
        "commonkit.browser-environment.v1",
        profile,
    )?)
}
pub fn pending_shards<'a>(
    shards: &'a [BrowserShard],
    checkpoints: &[Checkpoint],
) -> Vec<&'a BrowserShard> {
    let complete: BTreeSet<&str> = checkpoints
        .iter()
        .map(|checkpoint| checkpoint.stage.as_str())
        .collect();
    shards
        .iter()
        .filter(|shard| !complete.contains(shard.shard_id.as_str()))
        .collect()
}
pub fn aggregate(
    profile: &BrowserProfile,
    results: &[BrowserShardResult],
) -> Result<BrowserAggregate, BrowserError> {
    let environment = environment_digest(profile)?;
    let mut ordinals = BTreeSet::new();
    let mut artifacts = BTreeSet::new();
    for result in results {
        if result.environment_digest != environment {
            return Err(BrowserError::IncomparableEnvironment);
        }
        if !ordinals.insert(result.shard.ordinal) {
            return Err(BrowserError::DuplicateShard);
        }
        artifacts.extend(result.artifact_ids.iter().cloned());
    }
    let passed = results.iter().filter(|result| result.passed).count() as u32;
    let normalized: BTreeMap<u32, &BrowserShardResult> = results
        .iter()
        .map(|result| (result.shard.ordinal, result))
        .collect();
    let result_digest = digest_domain_json("commonkit.browser-aggregate.v1", &normalized)?;
    Ok(BrowserAggregate {
        environment_digest: environment,
        result_digest,
        shard_count: results.len() as u32,
        passed,
        failed: results.len() as u32 - passed,
        artifact_ids: artifacts.into_iter().collect(),
    })
}
#[derive(Debug, Error)]
pub enum BrowserError {
    #[error("browser environments are incomparable")]
    IncomparableEnvironment,
    #[error("duplicate browser shard ordinal")]
    DuplicateShard,
    #[error("browser contract failed")]
    Contract(#[from] commonkit_contracts::ContractError),
}
