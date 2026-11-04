//! One publication point for provider configuration and its credential identity.
//! Secrets are staged immutably in the existing store; only the document CAS
//! makes a staged version reachable. All cooperating readers/writers use the
//! stable gate, including retirement of the previous version. No await here.
use super::*;
use deepagent_persistence::fs_commit::{lock_operation_file, PathOperationGuard};

pub(super) struct ProviderDocument {
    pub state: CanvasProviderState,
    body: Option<String>,
}

pub(super) enum CredentialEdit<'a> {
    Preserve,
    Set(&'a str),
    Clear,
}

fn versioned_key(id: &str) -> Result<String> {
    let id = uuid::Uuid::parse_str(id)
        .map_err(|_| CoreError::Persistence("invalid canvas credential reference".into()))?;
    Ok(format!("canvas:credential:v1:{id}"))
}

// Discard a failed stage, including a backend that wrote and then returned an
// error. Process death can leave an unreachable encrypted record; it cannot
// publish that record or change an existing record's value.
struct PreparedSecret {
    store: Arc<dyn SecretStore>,
    name: Option<String>,
}

fn cleanup(store: &dyn SecretStore, name: &str) {
    if let Err(error) = store.delete(name) {
        // Do not echo arbitrary backend messages or credential contents.
        let category = match error {
            CoreError::Persistence(_) => "persistence",
            CoreError::Invalid(_) => "invalid_input",
            CoreError::NotFound(_) => "not_found",
            _ => "secret_backend",
        };
        tracing::warn!(
            credential_record = name,
            category,
            "canvas credential cleanup failed; unreachable record retained"
        );
    }
}

impl Drop for PreparedSecret {
    fn drop(&mut self) {
        if let Some(name) = &self.name {
            cleanup(self.store.as_ref(), name);
        }
    }
}

impl CanvasProviderService {
    pub(super) fn provider_gate(&self) -> Result<PathOperationGuard> {
        lock_operation_file(&self.db.data_root().join("canvas-provider-commit.lock")).map_err(
            |error| CoreError::Persistence(format!("acquire canvas provider commit gate: {error}")),
        )
    }

    pub(super) fn read_provider_document(&self) -> Result<ProviderDocument> {
        let doc = DocumentStore::new(&self.db).get(CANVAS_COLLECTION, CANVAS_PROVIDERS_ID)?;
        let body = doc.map(|doc| doc.body);
        let state = match body.as_deref() {
            None => CanvasProviderState::default(),
            Some(body) => serde_json::from_str(body).map_err(|error| {
                CoreError::Persistence(format!(
                    "invalid canvas provider state ({:?} at {}:{})",
                    error.classify(),
                    error.line(),
                    error.column()
                ))
            })?,
        };
        Ok(ProviderDocument { state, body })
    }

    pub(super) fn write_provider_document(&self, document: &ProviderDocument) -> Result<()> {
        let body = serde_json::to_string(&document.state)?;
        let store = DocumentStore::new(&self.db);
        let committed = match &document.body {
            Some(prior) => store.replace_body_if_unchanged(
                CANVAS_COLLECTION,
                CANVAS_PROVIDERS_ID,
                prior,
                &body,
                self.clock_now(),
            )?,
            None => store.insert_if_absent(
                CANVAS_COLLECTION,
                CANVAS_PROVIDERS_ID,
                &body,
                self.clock_now(),
            )?,
        };
        if !committed {
            return Err(CoreError::Persistence(
                "canvas providers changed before commit; retry the edit".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn credential_for(&self, provider: &CanvasProviderConfig) -> Result<Option<String>> {
        match &provider.credential_ref {
            Some(CanvasCredentialRef::Empty) => Ok(None),
            Some(CanvasCredentialRef::Secret { id }) => {
                let key = self.secrets.get(&versioned_key(id)?)?;
                match key.filter(|value| !value.trim().is_empty()) {
                    Some(key) => Ok(Some(key)),
                    None => Err(CoreError::Persistence(
                        "referenced canvas credential is unavailable; restore the credential record before use".into(),
                    )),
                }
            }
            // Only documents written by earlier versions may use the fixed slot.
            None => {
                let key = self.secrets.get(&provider_secret_key(&provider.id))?;
                Ok(key.filter(|value| !value.trim().is_empty()))
            }
        }
    }

    pub(super) fn retire_credential(&self, provider: &CanvasProviderConfig) {
        let name = match &provider.credential_ref {
            Some(CanvasCredentialRef::Empty) => return,
            Some(CanvasCredentialRef::Secret { id }) => match versioned_key(id) {
                Ok(name) => name,
                Err(_) => {
                    tracing::warn!("invalid retired canvas credential reference");
                    return;
                }
            },
            None => provider_secret_key(&provider.id),
        };
        cleanup(self.secrets.as_ref(), &name);
    }

    /// Gate is already held. Preserve does not change the current identity.
    /// No old key is overwritten, and no generation is published twice.
    pub(super) fn commit_provider(
        &self,
        mut document: ProviderDocument,
        mut provider: CanvasProviderConfig,
        previous: Option<&CanvasProviderConfig>,
        replacement: CredentialEdit<'_>,
    ) -> Result<CanvasProviderDto> {
        let current_key = previous
            .map(|old| self.credential_for(old))
            .transpose()?
            .flatten();
        let next_key = match replacement {
            CredentialEdit::Preserve => current_key.as_deref(),
            CredentialEdit::Set(key) => Some(key),
            CredentialEdit::Clear => None,
        };
        let changed = next_key != current_key.as_deref();
        provider.credential_version = previous
            .filter(|old| !changed && !old.credential_version.is_empty())
            .map(|old| old.credential_version.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut prepared = PreparedSecret {
            store: self.secrets.clone(),
            name: None,
        };
        provider.credential_ref = match next_key {
            None => Some(CanvasCredentialRef::Empty),
            Some(_)
                if !changed
                    && matches!(
                        previous.and_then(|old| old.credential_ref.as_ref()),
                        Some(CanvasCredentialRef::Secret { .. })
                    ) =>
            {
                previous.and_then(|old| old.credential_ref.clone())
            }
            Some(key) => {
                let id = uuid::Uuid::new_v4().to_string();
                let name = versioned_key(&id)?;
                prepared.name = Some(name.clone());
                self.secrets.set(&name, key)?;
                if self.secrets.get(&name)?.as_deref() != Some(key) {
                    return Err(CoreError::Persistence(
                        "canvas credential store did not persist the prepared value".into(),
                    ));
                }
                Some(CanvasCredentialRef::Secret { id })
            }
        };
        if let Some(slot) = document
            .state
            .providers
            .iter_mut()
            .find(|old| old.id == provider.id)
        {
            *slot = provider.clone();
        } else {
            document.state.providers.push(provider.clone());
        }
        let dto = provider.to_dto(next_key);
        self.write_provider_document(&document)?;
        // Only a successful CAS transfers ownership from the stage to the live
        // document. Cleanup failure never changes the active reference.
        prepared.name = None;
        if let Some(previous) = previous.filter(|old| old.credential_ref != provider.credential_ref)
        {
            self.retire_credential(previous);
        }
        tracing::info!("canvas provider configuration and credential reference committed");
        Ok(dto)
    }
}
