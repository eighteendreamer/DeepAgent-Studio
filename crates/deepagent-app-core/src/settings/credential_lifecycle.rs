//! Shared commit authority, not a second secret store. No lock crosses await.
//! The durable committing marker rejects half-published credentials after a
//! storage failure or process exit. Recovery is explicit, never a stale retry.
use super::*;
use deepagent_persistence::fs_commit::{lock_operation_file, PathOperationGuard};

const LIFECYCLE_ID: &str = "credential_lifecycle";
const LOCK_FILE: &str = "settings-credentials.lock";

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Ready,
    Committing,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    generation: String,
    phase: Phase,
}

struct Lifecycle {
    body: Option<String>,
    record: Option<Record>,
}

impl Lifecycle {
    fn generation(&self) -> Option<&str> {
        self.record
            .as_ref()
            .map(|record| record.generation.as_str())
    }

    fn phase(&self) -> Phase {
        self.record
            .as_ref()
            .map(|record| record.phase)
            .unwrap_or(Phase::Ready)
    }

    fn require_ready(&self) -> Result<()> {
        if matches!(self.phase(), Phase::Committing) {
            return Err(CoreError::Persistence(
                "credential update incomplete; sign in again or clear credentials".into(),
            ));
        }
        Ok(())
    }
}

// Deliberately no Debug: this capture contains a synthetic or real secret.
struct CredentialCapture {
    generation: Option<String>,
    key: String,
}

fn superseded() -> CoreError {
    CoreError::other("credential operation superseded by another sign-in or sign-out")
}

impl SettingsService {
    fn credential_gate(&self) -> Result<PathOperationGuard> {
        lock_operation_file(&self.db.data_root().join(LOCK_FILE)).map_err(|error| {
            CoreError::Persistence(format!("acquire credential commit gate: {error}"))
        })
    }

    fn load_lifecycle(&self) -> Result<Lifecycle> {
        let document = DocumentStore::new(&self.db).get(SETTINGS_COLLECTION, LIFECYCLE_ID)?;
        let Some(document) = document else {
            // Existing databases retain their current secret without migration.
            return Ok(Lifecycle {
                body: None,
                record: None,
            });
        };
        let record: Record = serde_json::from_str(&document.body).map_err(|error| {
            CoreError::Persistence(format!(
                "invalid credential lifecycle ({:?} at {}:{}); repair before using credentials",
                error.classify(),
                error.line(),
                error.column()
            ))
        })?;
        if record.version != 1 || uuid::Uuid::parse_str(&record.generation).is_err() {
            return Err(CoreError::Persistence(
                "unsupported credential lifecycle".into(),
            ));
        }
        Ok(Lifecycle {
            body: Some(document.body),
            record: Some(record),
        })
    }

    fn save_lifecycle(&self, expected: &Lifecycle, record: Record) -> Result<Lifecycle> {
        let body = serde_json::to_string(&record)?;
        let store = DocumentStore::new(&self.db);
        let now = Timestamp::from_millis(now_ms());
        let committed = match &expected.body {
            Some(prior) => store.replace_body_if_unchanged(
                SETTINGS_COLLECTION,
                LIFECYCLE_ID,
                prior,
                &body,
                now,
            )?,
            None => store.insert_if_absent(SETTINGS_COLLECTION, LIFECYCLE_ID, &body, now)?,
        };
        if !committed {
            return Err(CoreError::Persistence(
                "credential lifecycle changed unexpectedly; retry operation".into(),
            ));
        }
        Ok(Lifecycle {
            body: Some(body),
            record: Some(record),
        })
    }

    pub(super) fn begin_credential_change(&self) -> Result<String> {
        let _gate = self.credential_gate()?;
        let current = self.load_lifecycle()?;
        let generation = uuid::Uuid::new_v4().to_string();
        self.save_lifecycle(
            &current,
            Record {
                version: 1,
                generation: generation.clone(),
                phase: current.phase(),
            },
        )?;
        // A new intent revokes older publications without deleting the active
        // key if its own HTTP discovery subsequently fails.
        Ok(generation)
    }

    pub(super) fn complete_credential_change(
        &self,
        generation: &str,
        key: &str,
        catalog: ModelCatalog,
    ) -> Result<SettingsView> {
        let _gate = self.credential_gate()?;
        let current = self.load_lifecycle()?;
        if current.generation() != Some(generation) {
            return Err(superseded());
        }
        let settings = self.commit_credential(&current, generation, Some(key), || {
            let settings = self.merge_discovered_catalog(catalog, CatalogUpdate::Initialize)?;
            self.seed_harness_registry(&settings)?;
            Ok(settings)
        })?;
        tracing::info!("credential initialization committed");
        self.view_with_key(Some(key), &settings)
    }

    // Called only with the credential gate held. Sign-in, sign-out and legacy
    // import share one durable failure-closed publication state machine.
    fn commit_credential<T>(
        &self,
        current: &Lifecycle,
        generation: &str,
        key: Option<&str>,
        publish: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let pending = self.save_lifecycle(
            current,
            Record {
                version: 1,
                generation: generation.to_owned(),
                phase: Phase::Committing,
            },
        )?;
        // Any subsequent failure leaves durable recovery-required state. Do not
        // roll back to an older generation or silently use a partly written key.
        match key {
            Some(key) => self.secrets.set(API_KEY_NAME, key)?,
            None => self.secrets.delete(API_KEY_NAME)?,
        }
        if self.secrets.get(API_KEY_NAME)?.as_deref() != key {
            return Err(CoreError::other(
                "credential store did not persist the requested state",
            ));
        }
        let published = publish()?;
        self.save_lifecycle(
            &pending,
            Record {
                version: 1,
                generation: generation.to_owned(),
                phase: Phase::Ready,
            },
        )?;
        Ok(published)
    }

    fn capture_credential_refresh(&self) -> Result<CredentialCapture> {
        let _gate = self.credential_gate()?;
        let current = self.load_lifecycle()?;
        current.require_ready()?;
        let key = self
            .secrets
            .get(API_KEY_NAME)?
            .ok_or_else(|| CoreError::not_found("API key not set; initialize first"))?;
        Ok(CredentialCapture {
            generation: current.generation().map(ToOwned::to_owned),
            key,
        })
    }

    pub(super) async fn discover_for_refresh(&self) -> Result<SettingsView> {
        let captured = self.capture_credential_refresh()?;
        let catalog = ModelDiscovery::deepseek(self.transport.clone())
            .discover(&captured.key)
            .await?;
        let _gate = self.credential_gate()?;
        let current = self.load_lifecycle()?;
        current.require_ready()?;
        if current.generation() != captured.generation.as_deref()
            || self.secrets.get(API_KEY_NAME)?.as_deref() != Some(captured.key.as_str())
        {
            return Err(superseded());
        }
        let publication = self.merge_discovered_catalog(catalog, CatalogUpdate::Refresh);
        if publication.is_err() {
            // Do not echo provider identifiers, credentials or persisted fields.
            tracing::warn!("model catalog refresh not published; prior settings retained");
        }
        let settings = publication?;
        tracing::info!("model catalog refreshed without replacing saved role choices");
        // Discovery never writes/re-encrypts its captured credential.
        self.view_with_key(Some(&captured.key), &settings)
    }

    pub(super) fn read_current_credential(&self) -> Result<Option<String>> {
        let _gate = self.credential_gate()?;
        self.load_lifecycle()?.require_ready()?;
        self.secrets.get(API_KEY_NAME)
    }

    pub(super) fn current_credential_view(&self) -> Result<Option<SettingsView>> {
        let _gate = self.credential_gate()?;
        self.load_lifecycle()?.require_ready()?;
        let Some(settings) = self.load()? else {
            return Ok(None);
        };
        let key = self.secrets.get(API_KEY_NAME)?;
        self.view_with_key(key.as_deref(), &settings).map(Some)
    }

    pub(super) fn clear_current_credential(&self) -> Result<()> {
        let _gate = self.credential_gate()?;
        let current = self.load_lifecycle()?;
        let generation = uuid::Uuid::new_v4().to_string();
        self.commit_credential(&current, &generation, None, || Ok(()))?;
        tracing::info!("credential sign-out committed");
        Ok(())
    }

    /// Import only a never-owned legacy credential. A sign-in/sign-out intent
    /// is a durable tombstone even when the current SQLite key is absent.
    /// The desktop's compatibility importer must use this same owner, rather
    /// than restoring an old keychain value after another host signs out.
    pub fn import_legacy_api_key(&self, source: &dyn SecretStore) -> Result<bool> {
        let _gate = self.credential_gate()?;
        let current = self.load_lifecycle()?;
        if current.record.is_some() || self.secrets.get(API_KEY_NAME)?.is_some() {
            return Ok(false);
        }
        let Some(key) = source.get(API_KEY_NAME)? else {
            return Ok(false);
        };
        let generation = uuid::Uuid::new_v4().to_string();
        self.commit_credential(&current, &generation, Some(&key), || Ok(()))?;
        source.delete(API_KEY_NAME)?;
        tracing::info!("legacy credential import committed");
        Ok(true)
    }
}
