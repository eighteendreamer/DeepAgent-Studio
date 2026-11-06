//! H-223: discovery owns availability; committed settings own both role choices.
use super::*;
use crate::MemorySecretStore;
use async_trait::async_trait;
use deepagent_models::transport::{EventSink, TransportRequest};
use deepagent_models::{ModelRole, ResponseRequest};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

const FLASH: &str = "deepseek-v4-flash";
const PRO: &str = "deepseek-v4-pro";
const KEY: &str = "h223-synthetic-credential";

#[derive(Default)]
struct Secrets {
    inner: MemorySecretStore,
    writes: AtomicUsize,
}
impl SecretStore for Secrets {
    fn get(&self, name: &str) -> Result<Option<String>> {
        self.inner.get(name)
    }
    fn set(&self, name: &str, value: &str) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.set(name, value)
    }
    fn delete(&self, name: &str) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.delete(name)
    }
}

#[derive(Default)]
struct Discovery {
    body: Mutex<String>,
    armed: AtomicBool,
    started: Notify,
    release: Notify,
    wire: Mutex<Vec<String>>,
}
impl Discovery {
    fn models(&self, ids: &[&str]) {
        *self.body.lock().unwrap() = serde_json::json!({"data":ids.iter().map(|id| {
            serde_json::json!({"id":id,"object":"model","owned_by":"synthetic",
                "context_window":222_222,"max_output_tokens":12_345})
        }).collect::<Vec<_>>()})
        .to_string();
    }
}
#[async_trait]
impl HttpTransport for Discovery {
    async fn get_json(&self, _: &str, _: &str) -> Result<String> {
        // Freeze this response before waiting, like an in-flight discovery.
        let body = self.body.lock().unwrap().clone();
        if self.armed.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
            tokio::time::timeout(Duration::from_secs(10), self.release.notified())
                .await
                .map_err(|_| CoreError::other("H223 discovery fixture timed out"))?;
        }
        Ok(body)
    }
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::test_sequence::sink(sink);
        let body: serde_json::Value = serde_json::from_str(&request.body)?;
        self.wire
            .lock()
            .unwrap()
            .push(body["model"].as_str().unwrap().into());
        sink.on_event(r#"{"type":"response.completed","response":{"id":"fixture","object":"response","created_at":1,"model":"fixture","output":[],"status":"completed"}}"#)?;
        Ok(())
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    first: Arc<SettingsService>,
    second: SettingsService,
    discovery: Arc<Discovery>,
    secrets: Arc<Secrets>,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.sqlite");
        let discovery = Arc::new(Discovery::default());
        discovery.models(&[FLASH, PRO]);
        let secrets = Arc::new(Secrets::default());
        let first = Arc::new(SettingsService::new(
            Arc::new(Database::open(&path).unwrap()),
            discovery.clone(),
            secrets.clone(),
        ));
        first.initialize(KEY).await.unwrap();
        let second = SettingsService::new(
            Arc::new(Database::open(&path).unwrap()),
            discovery.clone(),
            secrets.clone(),
        );
        Self {
            directory,
            first,
            second,
            discovery,
            secrets,
        }
    }
    fn reopen(&self) -> SettingsService {
        SettingsService::new(
            Arc::new(Database::open(self.directory.path().join("settings.sqlite")).unwrap()),
            self.discovery.clone(),
            self.secrets.clone(),
        )
    }
    fn raw(&self) -> String {
        DocumentStore::new(&self.first.db)
            .get(SETTINGS_COLLECTION, SETTINGS_ID)
            .unwrap()
            .unwrap()
            .body
    }
    fn swapped(&self) {
        self.second.set_model(ModelRole::Chat, PRO).unwrap();
        self.second.set_model(ModelRole::Reasoner, FLASH).unwrap();
    }
    fn choices(&self, chat: &str, reasoner: &str) {
        for service in [&*self.first, &self.second, &self.reopen()] {
            let settings = service.load().unwrap().unwrap();
            assert_eq!(settings.catalog.chat_model, chat);
            assert_eq!(settings.catalog.reasoner_model, reasoner);
        }
    }
    async fn pending(&self) -> Pending {
        self.discovery.armed.store(true, Ordering::SeqCst);
        let first = self.first.clone();
        let pending = Pending {
            task: tokio::spawn(async move { first.refresh_models().await }),
            discovery: self.discovery.clone(),
        };
        tokio::time::timeout(Duration::from_secs(10), self.discovery.started.notified())
            .await
            .unwrap();
        pending
    }
}
struct Pending {
    task: tokio::task::JoinHandle<Result<SettingsView>>,
    discovery: Arc<Discovery>,
}
impl Pending {
    async fn finish(mut self) -> Result<SettingsView> {
        self.discovery.release.notify_one();
        tokio::time::timeout(Duration::from_secs(10), &mut self.task)
            .await
            .unwrap()
            .unwrap()
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.task.abort();
        self.discovery.release.notify_one();
    }
}

#[tokio::test]
async fn h223_same_reordered_and_expanded_catalog_keep_both_saved_roles() {
    for explicit in [false, true] {
        let f = Fixture::new().await;
        if explicit {
            f.swapped();
        }
        let expected = if explicit { (PRO, FLASH) } else { (FLASH, PRO) };
        let writes = f.secrets.writes.load(Ordering::SeqCst);
        for ids in [
            vec![FLASH, PRO],
            vec![PRO, FLASH],
            vec!["synthetic-chat", "synthetic-reasoner", PRO, FLASH],
        ] {
            f.discovery.models(&ids);
            let view = f.first.refresh_models().await.unwrap();
            assert_eq!(view.chat_model, expected.0);
            assert_eq!(view.reasoner_model, expected.1);
            assert_eq!(view.available_models, ids);
            let current = f.reopen().load().unwrap().unwrap();
            assert_eq!(current.catalog.available[0].context_window, Some(222_222));
            f.choices(expected.0, expected.1);
        }
        assert_eq!(f.secrets.writes.load(Ordering::SeqCst), writes);
    }
}

#[tokio::test]
async fn h223_removed_or_renamed_selected_id_rejects_the_entire_refresh() {
    for ids in [vec![FLASH], vec![PRO], vec![FLASH, "synthetic-pro-renamed"]] {
        let f = Fixture::new().await;
        let before = f.raw();
        f.discovery.models(&ids);
        let error = f.first.refresh_models().await.unwrap_err();
        assert!(matches!(error, CoreError::Invalid(_)), "{error}");
        assert!(error.to_string().contains("selected model"));
        assert!(!error.to_string().contains(KEY));
        assert_eq!(
            f.raw(),
            before,
            "no partial directory/pricing/date publication"
        );
        f.choices(FLASH, PRO);
        assert_eq!(f.secrets.writes.load(Ordering::SeqCst), 1);
        // Explicit re-initialization is different intent and may choose defaults.
        let view = f.first.initialize(KEY).await.unwrap();
        assert!(ids.contains(&view.chat_model.as_str()));
        assert!(ids.contains(&view.reasoner_model.as_str()));
    }
}

#[tokio::test]
async fn h223_refresh_uses_latest_other_host_choice_after_network_wait() {
    for removes_new_choice in [false, true] {
        let f = Fixture::new().await;
        // Both original choices remain in the one-model response at capture.
        f.second.set_model(ModelRole::Reasoner, FLASH).unwrap();
        if removes_new_choice {
            f.discovery.models(&[FLASH]);
        }
        let pending = f.pending().await;
        f.swapped();
        f.second
            .set_welcome_name("latest committed choice")
            .unwrap();
        f.second.set_sandbox_mode(SandboxMode::ReadOnly).unwrap();
        let before = f.raw();
        let refreshed = pending.finish().await;
        assert_eq!(refreshed.is_err(), removes_new_choice);
        if removes_new_choice {
            assert_eq!(f.raw(), before);
        }
        f.choices(PRO, FLASH);
        let current = f.reopen().load().unwrap().unwrap();
        assert_eq!(current.welcome_name, "latest committed choice");
        assert_eq!(current.sandbox_mode, SandboxMode::ReadOnly);
        assert_eq!(f.secrets.writes.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn h223_failed_discovery_or_commit_preserves_choices_and_all_public_fields() {
    let f = Fixture::new().await;
    f.swapped();
    let before = f.raw();
    for bad in ["not JSON", r#"{"data":[]}"#] {
        *f.discovery.body.lock().unwrap() = bad.into();
        assert!(f.first.refresh_models().await.is_err());
        assert_eq!(f.raw(), before);
    }
    f.discovery.models(&[PRO, FLASH, "synthetic-chat"]);
    f.first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER h223_reject BEFORE UPDATE ON documents
            WHEN NEW.collection='settings' AND NEW.id='app'
            BEGIN SELECT RAISE(ABORT, 'synthetic H223 write failure'); END;",
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert!(f.first.refresh_models().await.is_err());
    assert_eq!(f.raw(), before);
    f.choices(PRO, FLASH);
    f.first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch("DROP TRIGGER h223_reject")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    f.first.refresh_models().await.unwrap();
    f.choices(PRO, FLASH);
    assert_eq!(f.secrets.writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn h223_explicit_initialization_keeps_auto_selection_not_refresh_semantics() {
    let f = Fixture::new().await;
    f.swapped();
    f.second
        .set_welcome_name("preserved unrelated setting")
        .unwrap();
    let view = f.first.initialize(KEY).await.unwrap();
    assert_eq!(view.chat_model, FLASH);
    assert_eq!(view.reasoner_model, PRO);
    assert_eq!(
        f.reopen().welcome_name().unwrap(),
        "preserved unrelated setting"
    );
}

#[tokio::test]
async fn h223_refresh_can_create_a_missing_catalog_without_rewriting_legacy_key() {
    let f = Fixture::new().await;
    DocumentStore::new(&f.first.db)
        .delete(SETTINGS_COLLECTION, SETTINGS_ID)
        .unwrap();
    let writes = f.secrets.writes.load(Ordering::SeqCst);
    let view = f.first.refresh_models().await.unwrap();
    assert!(view.configured);
    f.choices(FLASH, PRO);
    assert_eq!(f.secrets.writes.load(Ordering::SeqCst), writes);
}

#[tokio::test]
async fn h223_both_role_consumers_send_the_preserved_id_on_real_model_client_wire() {
    let f = Fixture::new().await;
    f.swapped();
    f.first.refresh_models().await.unwrap();
    for (role, expected) in [(ModelRole::Chat, PRO), (ModelRole::Reasoner, FLASH)] {
        let (client, selected, _) =
            crate::model_runtime::build_model_client(&f.reopen(), f.discovery.clone(), role)
                .unwrap();
        assert_eq!(selected, expected);
        client
            .stream_response(ResponseRequest::new(
                selected,
                vec![deepagent_core::message::Message::user(
                    "synthetic role control",
                )],
            ))
            .await
            .unwrap();
    }
    assert_eq!(*f.discovery.wire.lock().unwrap(), [PRO, FLASH]);
}
