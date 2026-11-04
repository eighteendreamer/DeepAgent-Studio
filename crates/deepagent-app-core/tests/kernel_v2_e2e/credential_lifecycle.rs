//! H88: real ChatService/Kernel admission reads the current durable credential.
use super::*;
use deepagent_app_core::{SecretStore, SqliteSecretStore};
use tokio::sync::Notify;

const MODELS: &str =
    r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#;
const OLD: &str = "h88-kernel-synthetic-old";
const NEW: &str = "h88-kernel-synthetic-new";

struct Discovery {
    armed: AtomicBool,
    started: Notify,
    release: Notify,
}

#[async_trait]
impl HttpTransport for Discovery {
    async fn stream(&self, _: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        Err(CoreError::other("unexpected discovery streaming"))
    }
    async fn get_json(&self, _: &str, _: &str) -> Result<String> {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
            tokio::time::timeout(std::time::Duration::from_secs(10), self.release.notified())
                .await
                .map_err(|_| CoreError::other("H88 discovery timed out"))?;
        }
        Ok(MODELS.to_owned())
    }
}

#[derive(Default)]
struct Model(Mutex<Vec<String>>);

#[async_trait]
impl HttpTransport for Model {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        // Do not Debug/print the request or key, including failed assertions.
        self.0.lock().unwrap().push(request.api_key);
        emit_responses_events(sink, &response_text("H88 request accepted"))
    }
}

struct Pending(tokio::task::JoinHandle<Result<deepagent_app_core::SettingsView>>);

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn h88_next_kernel_request_never_uses_a_revoked_or_half_committed_credential() {
    for case in ["control", "logout", "replace", "aba", "failure"] {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let path = directory.path().join("settings.sqlite");
        let db = Arc::new(Database::open(&path).unwrap());
        let wrapping = Arc::new(MemorySecretStore::new());
        wrapping
            .set("deepagent_secret_wrap_key", "H88 synthetic kernel wrapping")
            .unwrap();
        let discovery = Arc::new(Discovery {
            armed: AtomicBool::new(false),
            started: Notify::new(),
            release: Notify::new(),
        });
        let first = Arc::new(SettingsService::new(
            db.clone(),
            discovery.clone(),
            Arc::new(SqliteSecretStore::new(db.clone(), wrapping.clone())),
        ));
        first.initialize(OLD).await.unwrap();
        let model = Arc::new(Model::default());
        let chat = ChatService::new(db.clone(), first.clone(), model.clone(), &workspace);
        let other_db = Arc::new(Database::open(&path).unwrap());
        let second = SettingsService::new(
            other_db.clone(),
            Arc::new(MockTransport::with_get_json(MODELS)),
            Arc::new(SqliteSecretStore::new(other_db, wrapping)),
        );
        discovery.armed.store(true, Ordering::SeqCst);
        let captured = first.clone();
        let mut pending = Pending(tokio::spawn(async move { captured.refresh_models().await }));
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            discovery.started.notified(),
        )
        .await
        .unwrap();
        let expected = match case {
            "control" => Some(OLD),
            "logout" => {
                second.clear_key().unwrap();
                None
            }
            "replace" => {
                second.initialize(NEW).await.unwrap();
                Some(NEW)
            }
            "aba" => {
                second.clear_key().unwrap();
                second.initialize(OLD).await.unwrap();
                Some(OLD)
            }
            "failure" => {
                db.with_conn(|connection| {
                    connection
                        .execute_batch(
                            "CREATE TRIGGER h88_kernel_reject BEFORE UPDATE ON documents
                    WHEN NEW.collection='settings' AND NEW.id='app'
                    BEGIN SELECT RAISE(ABORT, 'synthetic settings failure'); END;",
                        )
                        .map_err(|error| CoreError::Persistence(error.to_string()))
                })
                .unwrap();
                assert!(second.initialize(NEW).await.is_err());
                None
            }
            _ => unreachable!(),
        };
        discovery.release.notify_one();
        let refreshed = tokio::time::timeout(std::time::Duration::from_secs(10), &mut pending.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refreshed.is_ok(), case == "control");
        let run_id = format!("h88-{case}");
        let result = chat
            .run_in_session(
                "Reply briefly.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(&run_id),
                |_| {},
                |_| {},
            )
            .await;
        assert_eq!(result.is_ok(), expected.is_some());
        let requests = model.0.lock().unwrap();
        assert_eq!(requests.len(), usize::from(expected.is_some()));
        if let Some(expected) = expected {
            assert!(
                requests.iter().all(|key| key == expected),
                "wire credential must match the current committed identity"
            );
        }
        let terminals: Vec<_> = chat
            .run_events(&run_id, None)
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(
            terminals[0].data["kind"],
            if result.is_ok() {
                "succeeded"
            } else {
                "failed"
            }
        );
    }
}
