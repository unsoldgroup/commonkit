use std::{
    collections::BTreeSet,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimCategory {
    Identity,
    Locale,
    Communication,
    Workflow,
    Expertise,
    Accessibility,
    Custom,
}

impl ClaimCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Locale => "locale",
            Self::Communication => "communication",
            Self::Workflow => "workflow",
            Self::Expertise => "expertise",
            Self::Accessibility => "accessibility",
            Self::Custom => "custom",
        }
    }

    fn parse(value: &str) -> Result<Self, ProfileError> {
        match value {
            "identity" => Ok(Self::Identity),
            "locale" => Ok(Self::Locale),
            "communication" => Ok(Self::Communication),
            "workflow" => Ok(Self::Workflow),
            "expertise" => Ok(Self::Expertise),
            "accessibility" => Ok(Self::Accessibility),
            "custom" => Ok(Self::Custom),
            _ => Err(ProfileError::CorruptDatabase),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedView {
    pub loadout_id: String,
    pub project_id: String,
}

impl ScopedView {
    fn key(&self) -> String {
        format!("{}\0{}", self.loadout_id, self.project_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimInput {
    pub topic_key: String,
    pub category: ClaimCategory,
    pub text: String,
    pub views: Vec<ScopedView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Draft {
    pub id: String,
    pub expected_revision: u64,
    pub summary: String,
    pub claims: Vec<ClaimInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishedProfile {
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchResult {
    pub claim_id: String,
    pub topic_key: String,
    pub category: ClaimCategory,
    pub text: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Suggestion {
    pub id: String,
    pub view: ScopedView,
    pub claim: ClaimInput,
    pub evidence_quote: String,
    pub agent_id: String,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionDecision {
    Accept,
    Reject,
}

pub struct ProfileStore {
    connection: Connection,
    suppression_key: Vec<u8>,
}

impl Drop for ProfileStore {
    fn drop(&mut self) {
        self.suppression_key.zeroize();
    }
}

impl ProfileStore {
    pub fn open(path: impl AsRef<Path>, master_key: &[u8]) -> Result<Self, ProfileError> {
        if master_key.is_empty() {
            return Err(ProfileError::MissingKey);
        }
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut create = std::fs::OpenOptions::new();
        create.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            create.mode(0o600);
        }
        match create.open(path.as_ref()) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let metadata = std::fs::symlink_metadata(path.as_ref())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ProfileError::UnsafeDatabasePath);
        }
        let connection = Connection::open_with_flags(
            path.as_ref(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(path.as_ref(), std::fs::Permissions::from_mode(0o600))?;
        }
        let mut derived = Sha256::new();
        derived.update(b"commonkit.about-me.database.v1\0");
        derived.update(master_key);
        let database_key = derived.finalize();
        let mut key_literal = Zeroizing::new(String::from("x'"));
        for byte in database_key {
            use std::fmt::Write;
            write!(&mut *key_literal, "{byte:02x}").map_err(|_| ProfileError::KeySetup)?;
        }
        key_literal.push('\'');
        connection.pragma_update(None, "key", &*key_literal)?;
        connection.pragma_update(None, "cipher_memory_security", "ON")?;
        connection.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
        initialize_schema(&connection)?;

        let mut suppression = Sha256::new();
        suppression.update(b"commonkit.about-me.suppression.v1\0");
        suppression.update(master_key);
        Ok(Self {
            connection,
            suppression_key: suppression.finalize().to_vec(),
        })
    }

    pub fn revision(&self) -> Result<u64, ProfileError> {
        self.connection
            .query_row("SELECT revision FROM profile_meta WHERE singleton = 1", [], |row| {
                row.get(0)
            })
            .map_err(ProfileError::from)
    }

    pub fn create_draft(
        &mut self,
        expected_revision: u64,
        summary: impl Into<String>,
        claims: Vec<ClaimInput>,
    ) -> Result<Draft, ProfileError> {
        validate_claims(&claims)?;
        let draft = Draft {
            id: Uuid::new_v4().to_string(),
            expected_revision,
            summary: bounded(summary.into(), 4_096, "summary")?,
            claims,
        };
        let claims_json = serde_json::to_string(&draft.claims)?;
        self.connection.execute(
            "INSERT INTO drafts(id, expected_revision, summary, claims_json, created_at_ms)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![
                draft.id,
                draft.expected_revision,
                draft.summary,
                claims_json,
                now_ms()
            ],
        )?;
        Ok(draft)
    }

    pub fn publish_draft(
        &mut self,
        draft_id: &str,
        expected_revision: u64,
    ) -> Result<PublishedProfile, ProfileError> {
        let transaction = self.connection.transaction()?;
        let draft = load_draft(&transaction, draft_id)?;
        if draft.expected_revision != expected_revision {
            return Err(ProfileError::StaleRevision);
        }
        let current = current_revision(&transaction)?;
        if current != expected_revision {
            return Err(ProfileError::StaleRevision);
        }
        let revision = current + 1;
        let mut views = BTreeSet::new();
        for claim in &draft.claims {
            insert_claim(&transaction, claim, revision, None, "direct_user")?;
            views.extend(claim.views.iter().cloned());
        }
        for view in views {
            transaction.execute(
                "INSERT INTO summaries(view_key, summary, revision) VALUES(?1, ?2, ?3)
                 ON CONFLICT(view_key) DO UPDATE SET summary=excluded.summary, revision=excluded.revision",
                params![view.key(), draft.summary, revision],
            )?;
        }
        transaction.execute(
            "UPDATE profile_meta SET revision = ?1 WHERE singleton = 1",
            [revision],
        )?;
        transaction.execute("DELETE FROM drafts WHERE id = ?1", [draft_id])?;
        transaction.commit()?;
        Ok(PublishedProfile { revision })
    }

    pub fn summary(&self, view: &ScopedView) -> Result<Option<String>, ProfileError> {
        self.connection
            .query_row(
                "SELECT summary FROM summaries WHERE view_key = ?1",
                [view.key()],
                |row| row.get(0),
            )
            .optional()
            .map_err(ProfileError::from)
    }

    pub fn search(
        &mut self,
        view: &ScopedView,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, ProfileError> {
        let query = bounded(query.trim().to_owned(), 512, "query")?;
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let expression = format!("\"{}\"", query.replace('"', "\"\""));
        let mut statement = self.connection.prepare(
            "SELECT c.id, c.topic_key, c.category, c.text, c.revision
             FROM claims_fts f
             JOIN claims c ON c.id = f.claim_id
             JOIN claim_views v ON v.claim_id = c.id
             WHERE claims_fts MATCH ?1 AND c.active = 1 AND v.view_key = ?2
             ORDER BY rank LIMIT ?3",
        )?;
        let results = statement
            .query_map(params![expression, view.key(), limit.min(10) as u64], |row| {
                let category: String = row.get(2)?;
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    category,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .map(|row| {
                let (claim_id, topic_key, category, text, revision) = row?;
                Ok(SearchResult {
                    claim_id,
                    topic_key,
                    category: ClaimCategory::parse(&category)?,
                    text,
                    revision,
                })
            })
            .collect::<Result<Vec<_>, ProfileError>>()?;
        let now = now_ms();
        for result in &results {
            self.connection.execute(
                "INSERT INTO access_log(accessed_at_ms, view_key, claim_id) VALUES(?1, ?2, ?3)",
                params![now, view.key(), result.claim_id],
            )?;
        }
        Ok(results)
    }

    pub fn resolve_contradiction(
        &mut self,
        active_claim_id: &str,
        expected_revision: u64,
        replacement_text: &str,
        evidence_quote: &str,
    ) -> Result<PublishedProfile, ProfileError> {
        let replacement_text = bounded(replacement_text.trim().to_owned(), 8_192, "claim")?;
        bounded(evidence_quote.trim().to_owned(), 2_048, "evidence")?;
        let transaction = self.connection.transaction()?;
        if current_revision(&transaction)? != expected_revision {
            return Err(ProfileError::StaleRevision);
        }
        let (topic_key, category): (String, String) = transaction
            .query_row(
                "SELECT topic_key, category FROM claims WHERE id = ?1 AND active = 1",
                [active_claim_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(ProfileError::ClaimNotFound)?;
        let views = claim_views(&transaction, active_claim_id)?;
        transaction.execute(
            "UPDATE claims SET active = 0 WHERE id = ?1",
            [active_claim_id],
        )?;
        transaction.execute(
            "DELETE FROM claims_fts WHERE claim_id = ?1",
            [active_claim_id],
        )?;
        let revision = expected_revision + 1;
        insert_claim(
            &transaction,
            &ClaimInput {
                topic_key,
                category: ClaimCategory::parse(&category)?,
                text: replacement_text,
                views,
            },
            revision,
            Some(active_claim_id),
            "confirmed_contradiction",
        )?;
        transaction.execute(
            "UPDATE profile_meta SET revision = ?1 WHERE singleton = 1",
            [revision],
        )?;
        transaction.commit()?;
        Ok(PublishedProfile { revision })
    }

    pub fn suggest(
        &mut self,
        view: ScopedView,
        claim: ClaimInput,
        evidence_quote: &str,
        agent_id: &str,
    ) -> Result<Suggestion, ProfileError> {
        validate_claims(std::slice::from_ref(&claim))?;
        let evidence_quote = bounded(evidence_quote.trim().to_owned(), 2_048, "evidence")?;
        if evidence_quote.is_empty() {
            return Err(ProfileError::InvalidInput("evidence"));
        }
        let agent_id = bounded(agent_id.trim().to_owned(), 128, "agent")?;
        let fingerprint = suppression_fingerprint(&self.suppression_key, &view, &claim);
        if self
            .connection
            .query_row(
                "SELECT 1 FROM suppressions WHERE fingerprint = ?1",
                [&fingerprint],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            return Err(ProfileError::SuppressedSuggestion);
        }
        let suggestion = Suggestion {
            id: Uuid::new_v4().to_string(),
            view,
            claim,
            evidence_quote,
            agent_id,
            created_at_unix_ms: now_ms(),
        };
        self.connection.execute(
            "INSERT INTO suggestions(id, view_json, claim_json, evidence_quote, agent_id, created_at_ms)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                suggestion.id,
                serde_json::to_string(&suggestion.view)?,
                serde_json::to_string(&suggestion.claim)?,
                suggestion.evidence_quote,
                suggestion.agent_id,
                suggestion.created_at_unix_ms
            ],
        )?;
        Ok(suggestion)
    }

    pub fn pending_suggestions(&self) -> Result<Vec<Suggestion>, ProfileError> {
        let mut statement = self.connection.prepare(
            "SELECT id, view_json, claim_json, evidence_quote, agent_id, created_at_ms
             FROM suggestions ORDER BY created_at_ms, id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u64>(5)?,
                ))
            })?
            .map(|row| {
                let (id, view, claim, evidence_quote, agent_id, created_at_unix_ms) = row?;
                Ok(Suggestion {
                    id,
                    view: serde_json::from_str(&view)?,
                    claim: serde_json::from_str(&claim)?,
                    evidence_quote,
                    agent_id,
                    created_at_unix_ms,
                })
            })
            .collect()
    }

    pub fn decide_suggestion(
        &mut self,
        suggestion_id: &str,
        decision: SuggestionDecision,
    ) -> Result<Option<PublishedProfile>, ProfileError> {
        let suggestion = self
            .pending_suggestions()?
            .into_iter()
            .find(|suggestion| suggestion.id == suggestion_id)
            .ok_or(ProfileError::SuggestionNotFound)?;
        match decision {
            SuggestionDecision::Reject => {
                let fingerprint =
                    suppression_fingerprint(&self.suppression_key, &suggestion.view, &suggestion.claim);
                let transaction = self.connection.transaction()?;
                transaction.execute(
                    "INSERT OR IGNORE INTO suppressions(fingerprint, category, rejected_at_ms)
                     VALUES(?1, ?2, ?3)",
                    params![
                        fingerprint,
                        suggestion.claim.category.as_str(),
                        now_ms()
                    ],
                )?;
                transaction.execute("DELETE FROM suggestions WHERE id = ?1", [suggestion_id])?;
                transaction.commit()?;
                Ok(None)
            }
            SuggestionDecision::Accept => {
                let current = self.revision()?;
                let summary = self.summary(&suggestion.view)?.unwrap_or_default();
                let draft = self.create_draft(current, summary, vec![suggestion.claim])?;
                let published = self.publish_draft(&draft.id, current)?;
                self.connection
                    .execute("DELETE FROM suggestions WHERE id = ?1", [suggestion_id])?;
                Ok(Some(published))
            }
        }
    }

    pub fn purge_expired_metadata(&mut self, now_unix_ms: u64) -> Result<(), ProfileError> {
        const THIRTY_DAYS_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
        let cutoff = now_unix_ms.saturating_sub(THIRTY_DAYS_MS);
        self.connection
            .execute("DELETE FROM access_log WHERE accessed_at_ms < ?1", [cutoff])?;
        self.connection
            .execute("DELETE FROM suggestions WHERE created_at_ms < ?1", [cutoff])?;
        Ok(())
    }
}

fn initialize_schema(connection: &Connection) -> Result<(), ProfileError> {
    connection.execute_batch(
        "
        PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS profile_meta(
            singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
            revision INTEGER NOT NULL
        );
        INSERT OR IGNORE INTO profile_meta(singleton, revision) VALUES(1, 0);
        CREATE TABLE IF NOT EXISTS drafts(
            id TEXT PRIMARY KEY,
            expected_revision INTEGER NOT NULL,
            summary TEXT NOT NULL,
            claims_json TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS claims(
            id TEXT PRIMARY KEY,
            topic_key TEXT NOT NULL,
            category TEXT NOT NULL,
            text TEXT NOT NULL,
            revision INTEGER NOT NULL,
            active INTEGER NOT NULL,
            supersedes_id TEXT,
            source_kind TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS claim_views(
            claim_id TEXT NOT NULL REFERENCES claims(id) ON DELETE CASCADE,
            view_key TEXT NOT NULL,
            PRIMARY KEY(claim_id, view_key)
        );
        CREATE TABLE IF NOT EXISTS summaries(
            view_key TEXT PRIMARY KEY,
            summary TEXT NOT NULL,
            revision INTEGER NOT NULL
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS claims_fts USING fts5(
            claim_id UNINDEXED,
            text,
            topic_key,
            category
        );
        CREATE TABLE IF NOT EXISTS suggestions(
            id TEXT PRIMARY KEY,
            view_json TEXT NOT NULL,
            claim_json TEXT NOT NULL,
            evidence_quote TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS suppressions(
            fingerprint TEXT PRIMARY KEY,
            category TEXT NOT NULL,
            rejected_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS access_log(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            accessed_at_ms INTEGER NOT NULL,
            view_key TEXT NOT NULL,
            claim_id TEXT NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn validate_claims(claims: &[ClaimInput]) -> Result<(), ProfileError> {
    for claim in claims {
        if claim.topic_key.trim().is_empty()
            || claim.text.trim().is_empty()
            || claim.views.is_empty()
            || claim.views.iter().any(|view| {
                view.loadout_id.trim().is_empty() || view.project_id.trim().is_empty()
            })
        {
            return Err(ProfileError::InvalidInput("claim"));
        }
        bounded(claim.topic_key.clone(), 256, "topic_key")?;
        bounded(claim.text.clone(), 8_192, "claim")?;
    }
    Ok(())
}

fn load_draft(transaction: &Transaction<'_>, id: &str) -> Result<Draft, ProfileError> {
    transaction
        .query_row(
            "SELECT expected_revision, summary, claims_json FROM drafts WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
        .map(|(expected_revision, summary, claims)| -> Result<Draft, ProfileError> {
            Ok(Draft {
                id: id.into(),
                expected_revision,
                summary,
                claims: serde_json::from_str(&claims)?,
            })
        })
        .transpose()?
        .ok_or(ProfileError::DraftNotFound)
}

fn current_revision(transaction: &Transaction<'_>) -> Result<u64, ProfileError> {
    transaction
        .query_row("SELECT revision FROM profile_meta WHERE singleton = 1", [], |row| {
            row.get(0)
        })
        .map_err(ProfileError::from)
}

fn insert_claim(
    transaction: &Transaction<'_>,
    claim: &ClaimInput,
    revision: u64,
    supersedes_id: Option<&str>,
    source_kind: &str,
) -> Result<String, ProfileError> {
    let id = Uuid::new_v4().to_string();
    transaction.execute(
        "INSERT INTO claims(id, topic_key, category, text, revision, active, supersedes_id, source_kind, created_at_ms)
         VALUES(?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8)",
        params![
            id,
            claim.topic_key,
            claim.category.as_str(),
            claim.text,
            revision,
            supersedes_id,
            source_kind,
            now_ms()
        ],
    )?;
    for view in &claim.views {
        transaction.execute(
            "INSERT INTO claim_views(claim_id, view_key) VALUES(?1, ?2)",
            params![id, view.key()],
        )?;
    }
    transaction.execute(
        "INSERT INTO claims_fts(claim_id, text, topic_key, category) VALUES(?1, ?2, ?3, ?4)",
        params![id, claim.text, claim.topic_key, claim.category.as_str()],
    )?;
    Ok(id)
}

fn claim_views(
    transaction: &Transaction<'_>,
    claim_id: &str,
) -> Result<Vec<ScopedView>, ProfileError> {
    let mut statement =
        transaction.prepare("SELECT view_key FROM claim_views WHERE claim_id = ?1 ORDER BY view_key")?;
    statement
        .query_map([claim_id], |row| row.get::<_, String>(0))?
        .map(|row| {
            let key = row?;
            let (loadout_id, project_id) =
                key.split_once('\0').ok_or(ProfileError::CorruptDatabase)?;
            Ok(ScopedView {
                loadout_id: loadout_id.into(),
                project_id: project_id.into(),
            })
        })
        .collect()
}

fn suppression_fingerprint(key: &[u8], view: &ScopedView, claim: &ClaimInput) -> String {
    let mut digest = Sha256::new();
    digest.update(key);
    digest.update(view.key());
    digest.update(b"\0");
    digest.update(claim.topic_key.trim().to_lowercase());
    digest.update(b"\0");
    digest.update(claim.text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase());
    format!("{:x}", digest.finalize())
}

fn bounded(value: String, maximum: usize, field: &'static str) -> Result<String, ProfileError> {
    if value.len() > maximum {
        Err(ProfileError::InvalidInput(field))
    } else {
        Ok(value)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("profile key is required")]
    MissingKey,
    #[error("profile key setup failed")]
    KeySetup,
    #[error("profile database path is not a regular file")]
    UnsafeDatabasePath,
    #[error("invalid {0}")]
    InvalidInput(&'static str),
    #[error("profile changed since this operation began")]
    StaleRevision,
    #[error("draft was not found")]
    DraftNotFound,
    #[error("claim was not found")]
    ClaimNotFound,
    #[error("suggestion was not found")]
    SuggestionNotFound,
    #[error("the user previously rejected this suggestion")]
    SuppressedSuggestion,
    #[error("profile database is corrupt")]
    CorruptDatabase,
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
