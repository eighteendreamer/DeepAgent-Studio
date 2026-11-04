//! One commit boundary for public settings. A cache hit still checks the
//! authoritative document identity; a cached projection is never a write base.

use super::*;

// Interactive edits must fail explicitly rather than spin under a busy writer.
pub(super) const MAX_COMMIT_ATTEMPTS: usize = 5;

fn invalid_document(error: serde_json::Error) -> CoreError {
    // Preserve the failure class/location, never a serde message that can echo
    // the persisted value (settings may contain private hook/config content).
    CoreError::Persistence(format!(
        "invalid persisted settings ({:?} at {}:{}); repair before writing",
        error.classify(),
        error.line(),
        error.column()
    ))
}

fn decode(body: &str) -> Result<(serde_json::Value, AppSettings, usize)> {
    let mut stored: serde_json::Value = serde_json::from_str(body).map_err(invalid_document)?;
    if !stored.is_object() {
        return Err(CoreError::Persistence(
            "persisted settings must be an object; repair before writing".into(),
        ));
    }
    let changes = migrate_persisted_reasoning(&mut stored);
    let settings = serde_json::from_value(stored.clone()).map_err(invalid_document)?;
    Ok((stored, settings, changes))
}

fn conflict() -> CoreError {
    CoreError::Persistence("settings kept changing; retry the operation".into())
}

impl SettingsService {
    pub(super) fn load_current_settings(&self) -> Result<Option<AppSettings>> {
        let mut cache = self
            .settings_cache
            .lock()
            .map_err(|_| CoreError::other("settings cache lock poisoned"))?;
        let store = DocumentStore::new(&self.db);
        for _ in 0..MAX_COMMIT_ATTEMPTS {
            let Some(doc) = store.get(SETTINGS_COLLECTION, SETTINGS_ID)? else {
                *cache = None;
                return Ok(None);
            };
            if let Some((body, settings)) = &*cache {
                if body == &doc.body {
                    return Ok(Some(settings.clone()));
                }
            }
            let (stored, settings, changes) = decode(&doc.body)?;
            let body = if changes > 0 {
                let body = serde_json::to_string(&stored)?;
                if !store.replace_body_if_unchanged(
                    SETTINGS_COLLECTION,
                    SETTINGS_ID,
                    &doc.body,
                    &body,
                    Timestamp::from_millis(now_ms()),
                )? {
                    continue;
                }
                tracing::info!(
                    migrated_fields = changes,
                    "migrated persisted reasoning settings"
                );
                body
            } else {
                doc.body
            };
            *cache = Some((body, settings.clone()));
            return Ok(Some(settings));
        }
        Err(conflict())
    }

    /// The operation must be deterministic, side-effect-free and must not
    /// re-enter this service: a cross-host conflict reapplies it to fresh data.
    /// No network/secret-store work is allowed inside this publication gate.
    pub(super) fn edit_settings(
        &self,
        edit: impl Fn(Option<AppSettings>) -> Result<AppSettings>,
    ) -> Result<AppSettings> {
        let mut cache = self
            .settings_cache
            .lock()
            .map_err(|_| CoreError::other("settings cache lock poisoned"))?;
        let store = DocumentStore::new(&self.db);
        for attempt in 0..MAX_COMMIT_ATTEMPTS {
            let doc = store.get(SETTINGS_COLLECTION, SETTINGS_ID)?;
            let (mut stored, prior) = match &doc {
                Some(doc) => {
                    let (stored, settings, _) = decode(&doc.body)?;
                    (stored, Some(settings))
                }
                None => (serde_json::json!({}), None),
            };
            let settings = edit(prior)?;
            // Extend the current object, rather than rebuilding the document:
            // fields unknown to this version must survive ordinary edits.
            let serde_json::Value::Object(fields) = serde_json::to_value(&settings)? else {
                return Err(CoreError::other(
                    "settings serializer must produce an object",
                ));
            };
            let object = stored.as_object_mut().ok_or_else(|| {
                CoreError::Persistence(
                    "persisted settings must be an object; repair before writing".into(),
                )
            })?;
            object.extend(fields);
            let body = serde_json::to_string(&stored)?;
            let now = Timestamp::from_millis(now_ms());
            let committed = match &doc {
                Some(doc) => store.replace_body_if_unchanged(
                    SETTINGS_COLLECTION,
                    SETTINGS_ID,
                    &doc.body,
                    &body,
                    now,
                )?,
                None => store.insert_if_absent(SETTINGS_COLLECTION, SETTINGS_ID, &body, now)?,
            };
            if committed {
                *cache = Some((body, settings.clone()));
                return Ok(settings);
            }
            tracing::debug!(
                attempt = attempt + 1,
                "settings changed in another host; retrying edit"
            );
        }
        Err(conflict())
    }

    pub(super) fn update_settings(
        &self,
        update: impl Fn(&mut AppSettings) -> Result<()>,
    ) -> Result<AppSettings> {
        self.edit_settings(|prior| {
            let mut settings =
                prior.ok_or_else(|| CoreError::not_found("settings not initialized"))?;
            update(&mut settings)?;
            Ok(settings)
        })
    }
}
