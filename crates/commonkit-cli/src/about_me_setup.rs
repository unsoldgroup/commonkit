use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use commonkit_about_me::{ClaimCategory, ClaimInput, ProfileStore, ScopedView};
use commonkit_platform::{PrivatePathKind, ensure_private_path};
use rand::RngCore;
use serde_json::{Map, Value, json};
use thiserror::Error;
use zeroize::Zeroizing;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupAnswers {
    pub name: String,
    pub explanation_style: String,
    pub decision_style: String,
    pub tools: String,
    pub constraints: String,
    pub never_assume: String,
}

#[derive(Clone, Debug)]
pub struct SetupRequest {
    pub config_directory: PathBuf,
    pub state_directory: PathBuf,
    pub loadout_id: String,
    pub project_id: String,
    pub agent_id: String,
    pub answers: SetupAnswers,
    pub approved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupOutcome {
    pub database_path: PathBuf,
    pub key_path: PathBuf,
    pub revision: u64,
    pub summary: String,
}

pub fn setup_profile(request: SetupRequest) -> Result<SetupOutcome, SetupError> {
    if !request.approved {
        return Err(SetupError::NotApproved);
    }
    for value in [&request.loadout_id, &request.project_id, &request.agent_id] {
        if value.trim().is_empty() {
            return Err(SetupError::InvalidScope);
        }
    }

    let config_path = request.config_directory.join("headless.json");
    let mut config = read_config(&config_path)?;
    fs::create_dir_all(&request.config_directory)?;
    fs::create_dir_all(&request.state_directory)?;
    ensure_private_path(&request.config_directory, PrivatePathKind::Directory)?;
    ensure_private_path(&request.state_directory, PrivatePathKind::Directory)?;

    let key_path = request.config_directory.join("about-me.key");
    let key = load_or_create_key(&key_path)?;
    let database_path = request.state_directory.join("about-me.sqlite");
    let view = ScopedView {
        loadout_id: request.loadout_id.clone(),
        project_id: request.project_id.clone(),
    };
    let claims = claims_from_answers(&request.answers, &view);
    let summary = summary_from_answers(&request.answers);
    let mut store = ProfileStore::open(&database_path, &key)?;
    let revision = store.revision()?;
    if revision != 0 {
        return Err(SetupError::AlreadyConfigured);
    }
    let draft = store.create_draft(revision, summary.clone(), claims)?;
    let published = store.publish_draft(&draft.id, revision)?;

    config.insert(
        "aboutMe".into(),
        json!({
            "database": database_path,
            "keyReference": format!("file://{}", key_path.display()),
            "loadoutId": request.loadout_id,
            "projectId": request.project_id,
            "agentId": request.agent_id
        }),
    );
    write_private_json(&config_path, &Value::Object(config))?;

    Ok(SetupOutcome {
        database_path,
        key_path,
        revision: published.revision,
        summary,
    })
}

fn claims_from_answers(answers: &SetupAnswers, view: &ScopedView) -> Vec<ClaimInput> {
    [
        ("identity.name", ClaimCategory::Identity, &answers.name),
        (
            "communication.explanation_style",
            ClaimCategory::Communication,
            &answers.explanation_style,
        ),
        (
            "workflow.decision_style",
            ClaimCategory::Workflow,
            &answers.decision_style,
        ),
        ("expertise.tools", ClaimCategory::Expertise, &answers.tools),
        (
            "workflow.constraints",
            ClaimCategory::Workflow,
            &answers.constraints,
        ),
        (
            "communication.never_assume",
            ClaimCategory::Communication,
            &answers.never_assume,
        ),
    ]
    .into_iter()
    .filter_map(|(topic_key, category, text)| {
        let text = text.trim();
        (!text.is_empty()).then(|| ClaimInput {
            topic_key: topic_key.into(),
            category,
            text: text.into(),
            views: vec![view.clone()],
        })
    })
    .collect()
}

fn summary_from_answers(answers: &SetupAnswers) -> String {
    let mut parts = Vec::new();
    if !answers.name.trim().is_empty() {
        parts.push(format!("Call me {}.", answers.name.trim()));
    }
    if !answers.explanation_style.trim().is_empty() {
        parts.push(answers.explanation_style.trim().to_owned());
    }
    if !answers.decision_style.trim().is_empty() {
        parts.push(answers.decision_style.trim().to_owned());
    }
    parts.join(" ")
}

fn read_config(path: &Path) -> Result<Map<String, Value>, SetupError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)?
            .as_object()
            .cloned()
            .ok_or(SetupError::InvalidConfig),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(error) => Err(error.into()),
    }
}

fn load_or_create_key(path: &Path) -> Result<Zeroizing<Vec<u8>>, SetupError> {
    match fs::read(path) {
        Ok(bytes) if bytes.len() == 32 => return Ok(Zeroizing::new(bytes)),
        Ok(_) => return Err(SetupError::InvalidKey),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut key = Zeroizing::new(vec![0_u8; 32]);
    rand::rng().fill_bytes(&mut key);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&key)?;
    file.sync_all()?;
    ensure_private_path(path, PrivatePathKind::File)?;
    Ok(key)
}

fn write_private_json(path: &Path, value: &Value) -> Result<(), SetupError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let stage = path.with_extension("json.about-me-stage");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&stage)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    ensure_private_path(&stage, PrivatePathKind::File)?;
    fs::rename(stage, path)?;
    ensure_private_path(path, PrivatePathKind::File)?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum SetupError {
    #[error("profile setup was not approved; nothing was saved")]
    NotApproved,
    #[error("profile scope must name a loadout, project, and agent")]
    InvalidScope,
    #[error("the existing CommonKit configuration is not a JSON object")]
    InvalidConfig,
    #[error("the existing About Me key is not valid")]
    InvalidKey,
    #[error("an About Me profile is already configured")]
    AlreadyConfigured,
    #[error(transparent)]
    Profile(#[from] commonkit_about_me::ProfileError),
    #[error(transparent)]
    Platform(#[from] commonkit_platform::PlatformError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
