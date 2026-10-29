//! 重启恢复验收：把数据库关掉再按同一路径打开，画布的图、模型绑定、密钥、
//! 制品和偏好文档必须都还在原样；同时证明明文密钥只以密文形式存在。
//!
//! 这里的“重启”= 丢弃全部 store 与数据库句柄，重新 `Database::open(path)`。
//! 密钥包裹键存放在注入的 wrapping store 里，等价于重启后系统钥匙串仍可用。

use std::sync::Arc;

use deepagent_app_core::canvas_artifact_service::{CanvasArtifactImport, CanvasArtifactService};
use deepagent_app_core::canvas_preferences::CanvasPreferencesStore;
use deepagent_app_core::canvas_provider_service::{
    CanvasModelConfig, CanvasProviderInput, CanvasProviderService, CanvasScenario,
    CanvasScenarioBinding,
};
use deepagent_app_core::canvas_workflow_store::{current_workflow_id, CanvasWorkflowStore};
use deepagent_app_core::secret_store::{MemorySecretStore, SqliteSecretStore};
use deepagent_persistence::artifact_store::ArtifactKind;
use deepagent_persistence::Database;
use serde_json::{json, Value};

/// 合成 key：只用于验证加密存储与重启解密，不是任何真实凭据。
const PLAINTEXT_KEY: &str = "sk-canvas-restart-test-4b1e7c02a9d84f53";

struct TempRoot {
    dir: std::path::PathBuf,
}

impl TempRoot {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "deepagent_canvas_restart_{tag}_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("temp root");
        Self { dir }
    }

    fn db_path(&self) -> std::path::PathBuf {
        self.dir.join("app.db")
    }

    fn files_dir(&self) -> std::path::PathBuf {
        let dir = self.dir.join("files").join("artifacts");
        std::fs::create_dir_all(&dir).expect("artifact root");
        dir
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn model(id: &str, scenarios: Vec<CanvasScenario>) -> CanvasModelConfig {
    CanvasModelConfig {
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        enabled: true,
        scenarios,
        priority: 0,
        max_reference_images: None,
    }
}

/// 画布图的骨架。`promptProfileVersion` 是节点创建时固定的提示词版本，
/// 重启后必须仍然带着同一个版本，否则同一张图会跑到不同的提示词上。
fn graph() -> Value {
    json!({
        "version": 1,
        "mode": "creative",
        "nodes": [{
            "id": "image-1",
            "type": "creative-image",
            "position": {"x": 120, "y": 80},
            "data": {
                "kind": "image-gen",
                "prompt": "一杯冰美式放在木桌上",
                "imageModel": "cvp-1::gpt-image-2",
                "promptProfileVersion": "creative.image-gen.v1",
            },
        }],
        "edges": [],
    })
}

fn png_data_url() -> String {
    use base64::Engine;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode([0x89u8, b'P', b'N', b'G', 1, 2, 3])
    )
}

#[test]
fn h252_artifact_namespaces_survive_reopen_and_concurrent_foreign_deletes() {
    let root = TempRoot::new("artifact_namespaces");
    let scopes = [None, Some("ws-a"), Some("ws-b")];
    let items = {
        let service = CanvasArtifactService::new(
            root.files_dir(),
            Arc::new(Database::open(root.db_path()).unwrap()),
        )
        .unwrap();
        scopes.map(|scope| {
            service
                .import_bytes(
                    ArtifactKind::Video,
                    Some("video/mp4"),
                    b"synthetic scoped video",
                    scope,
                )
                .unwrap()
        })
    };
    // Every worker opens a separate SQLite connection after the writer closes.
    let barrier = std::sync::Barrier::new(scopes.len());
    std::thread::scope(|threads| {
        let mut workers = Vec::new();
        for (index, caller) in scopes.iter().copied().enumerate() {
            let root = &root;
            let items = &items;
            let barrier = &barrier;
            workers.push(threads.spawn(move || {
                let service = CanvasArtifactService::new(
                    root.files_dir(),
                    Arc::new(Database::open(root.db_path()).unwrap()),
                )
                .unwrap();
                barrier.wait();
                for _ in 0..12 {
                    assert_eq!(service.list(caller, None).unwrap().len(), 1);
                    for (owner, item) in items.iter().enumerate() {
                        if owner == index {
                            assert_eq!(
                                service.read_bytes(&item.id, caller).unwrap().unwrap(),
                                b"synthetic scoped video"
                            );
                            service.video_file_path(&item.uri, caller).unwrap();
                        } else {
                            assert!(service.record(&item.id, caller).unwrap().is_none());
                            assert!(service.resolve_for_provider(&item.uri, caller).is_err());
                            assert!(!service.delete(&item.id, caller).unwrap());
                        }
                    }
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
    });
    let reopened = CanvasArtifactService::new(
        root.files_dir(),
        Arc::new(Database::open(root.db_path()).unwrap()),
    )
    .unwrap();
    for (owner, item) in scopes.into_iter().zip(items) {
        assert_eq!(
            reopened
                .record(&item.id, owner)
                .unwrap()
                .unwrap()
                .workspace_id
                .as_deref(),
            owner
        );
        assert_eq!(
            reopened.read_bytes(&item.id, owner).unwrap().unwrap(),
            b"synthetic scoped video"
        );
        assert!(reopened.delete(&item.id, owner).unwrap());
        assert!(reopened.record(&item.id, owner).unwrap().is_none());
    }
}

#[test]
fn h250_reopened_media_preserves_expected_digest_and_rejects_changed_content() {
    use deepagent_core::error::CoreError;
    let root = TempRoot::new("h250_integrity");
    let items = {
        let service = CanvasArtifactService::new(
            root.files_dir(),
            Arc::new(Database::open(root.db_path()).unwrap()),
        )
        .unwrap();
        [
            ArtifactKind::Image,
            ArtifactKind::Audio,
            ArtifactKind::Video,
        ]
        .map(|kind| {
            service
                .import_bytes(kind, None, b"original media", Some("ws-a"))
                .unwrap()
        })
    };
    std::fs::write(&items[0].path, b"changed media!").unwrap();
    std::fs::remove_file(&items[1].path).unwrap();
    let reopened = CanvasArtifactService::new(
        root.files_dir(),
        Arc::new(Database::open(root.db_path()).unwrap()),
    )
    .unwrap();
    for item in &items[..2] {
        let record = reopened.record(&item.id, Some("ws-a")).unwrap().unwrap();
        match reopened.read_bytes(&item.id, Some("ws-a")).unwrap_err() {
            CoreError::ArtifactIntegrityMismatch {
                artifact_id,
                expected_digest,
                ..
            } => {
                assert_eq!(artifact_id, item.id);
                assert_eq!(expected_digest, record.digest);
            }
            other => panic!("unexpected integrity result: {other}"),
        }
        assert!(reopened
            .read_bytes(&item.id, Some("ws-b"))
            .unwrap()
            .is_none());
        assert_eq!(
            reopened.record(&item.id, Some("ws-a")).unwrap(),
            Some(record)
        );
    }
    assert_eq!(
        reopened
            .read_bytes(&items[2].id, Some("ws-a"))
            .unwrap()
            .unwrap(),
        b"original media"
    );
    reopened
        .video_file_path(&items[2].uri, Some("ws-a"))
        .unwrap();
}

#[test]
fn named_workflow_operations_preserve_live_slots_across_independent_handles_and_reopen() {
    let root = TempRoot::new("workflow_namespace");
    let snapshots = {
        let first = CanvasWorkflowStore::new(Arc::new(Database::open(root.db_path()).unwrap()));
        let second = CanvasWorkflowStore::new(Arc::new(Database::open(root.db_path()).unwrap()));
        let mut snapshots = Vec::new();
        for mode in ["creative", "professional"] {
            for workspace in [None, Some("ws-a"), Some("ws-b")] {
                let id = current_workflow_id(mode, workspace).unwrap();
                let live = json!({"nodes":[{"id":"live","data":{"owner":id}}],"edges":[]});
                first.write_current(mode, workspace, &live, None).unwrap();
                let before = first.read_current(mode, workspace).unwrap();
                // Repeat import/update/delete attempts through public production
                // methods, without mutating the database behind the service.
                for _ in 0..2 {
                    let error = second
                        .save_workflow(
                            Some(&id),
                            "collision",
                            mode,
                            workspace,
                            &json!({"nodes":[]}),
                        )
                        .unwrap_err();
                    assert!(error.to_string().contains("reserved_workflow_id"));
                    assert!(second.load_workflow(&id, None).is_err());
                    assert!(second.delete_workflow(&id, None).is_err());
                }
                assert_eq!(second.read_current(mode, workspace).unwrap(), before);
                // The H-244 conflict-copy path must still save/load a fresh name.
                let copy = second
                    .save_workflow(None, "preserved copy", mode, workspace, &live)
                    .unwrap();
                assert!(copy.id.starts_with("wf-"));
                assert_eq!(
                    second
                        .load_workflow(&copy.id, workspace)
                        .unwrap()
                        .unwrap()
                        .graph,
                    live
                );
                snapshots.push((mode, workspace, before, copy.id));
            }
        }
        for workspace in [None, Some("ws-a"), Some("ws-b")] {
            assert_eq!(first.list_workflows(workspace).unwrap().len(), 2);
        }
        snapshots
    };
    // Both original database connections have been closed, not merely reused.
    let reopened = CanvasWorkflowStore::new(Arc::new(Database::open(root.db_path()).unwrap()));
    for (mode, workspace, expected, named_id) in snapshots {
        assert_eq!(reopened.read_current(mode, workspace).unwrap(), expected);
        assert_eq!(
            reopened
                .load_workflow(&named_id, workspace)
                .unwrap()
                .unwrap()
                .graph,
            expected.graph.clone().unwrap()
        );
        for other in [None, Some("ws-a"), Some("ws-b")]
            .into_iter()
            .filter(|other| *other != workspace)
        {
            assert!(reopened.load_workflow(&named_id, other).unwrap().is_none());
            assert!(!reopened.delete_workflow(&named_id, other).unwrap());
            assert!(reopened
                .save_workflow(Some(&named_id), "cross-scope", mode, other, &json!({}))
                .is_err());
        }
        assert!(reopened.delete_workflow(&named_id, workspace).unwrap());
        assert_eq!(reopened.read_current(mode, workspace).unwrap(), expected);
        let updated = json!({"nodes":[{"id":"newer"}],"edges":[]});
        reopened
            .write_current(mode, workspace, &updated, expected.revision.as_deref())
            .unwrap();
        let stale = reopened.write_current(
            mode,
            workspace,
            &json!({"nodes":[]}),
            expected.revision.as_deref(),
        );
        assert_eq!(stale.unwrap_err().code, "revision_conflict");
        assert_eq!(
            reopened.read_current(mode, workspace).unwrap().graph,
            Some(updated)
        );
    }
    for workspace in [None, Some("ws-a"), Some("ws-b")] {
        assert!(reopened.list_workflows(workspace).unwrap().is_empty());
    }
}

#[test]
fn named_workflow_deletions_keep_their_owner_and_graph_after_database_reopen() {
    use deepagent_app_core::canvas_workflow_store::CANVAS_WORKFLOW_COLLECTION;
    use deepagent_persistence::document_store::DocumentStore;

    let root = TempRoot::new("named_tombstones");
    let records = {
        let db = Arc::new(Database::open(root.db_path()).unwrap());
        let workflows = CanvasWorkflowStore::new(db.clone());
        [None, Some("ws-a"), Some("ws-b")].map(|scope| {
            let graph = json!({"owner":scope,"nodes":[{"id":"retained"}]});
            let item = workflows
                .save_workflow(None, "retained", "creative", scope, &graph)
                .unwrap();
            assert!(workflows.delete_workflow(&item.id, scope).unwrap());
            let raw = DocumentStore::new(&db)
                .get(CANVAS_WORKFLOW_COLLECTION, &item.id)
                .unwrap()
                .unwrap();
            (item.id, scope, graph, raw)
        })
    };
    let db = Arc::new(Database::open(root.db_path()).unwrap());
    let workflows = CanvasWorkflowStore::new(db.clone());
    for (id, owner, graph, retained) in records {
        for scope in [None, Some("ws-a"), Some("ws-b")] {
            assert!(workflows.load_workflow(&id, scope).unwrap().is_none());
            assert!(workflows.list_workflows(scope).unwrap().is_empty());
            assert!(!workflows.delete_workflow(&id, scope).unwrap());
            assert!(workflows
                .save_workflow(Some(&id), "reclaim", "creative", scope, &json!({}))
                .is_err());
        }
        let raw = DocumentStore::new(&db)
            .get(CANVAS_WORKFLOW_COLLECTION, &id)
            .unwrap()
            .unwrap();
        assert_eq!(
            raw, retained,
            "rejected operations cannot mutate tombstones"
        );
        let body: Value = serde_json::from_str(&raw.body).unwrap();
        assert_eq!(body["workspaceId"], json!(owner));
        assert_eq!(body["graph"], graph);
        assert!(body["deletedAt"].is_i64());
    }
}

#[test]
fn every_canvas_document_survives_a_reopen_of_the_same_database() {
    let root = TempRoot::new("round_trip");
    let wrapping = Arc::new(MemorySecretStore::default());

    // 第一次启动：写入图、绑定、密钥、制品和偏好。
    let (artifact_uri, artifact_bytes) = {
        let db = Arc::new(Database::open(root.db_path()).expect("open db"));
        let providers = CanvasProviderService::new(
            db.clone(),
            Arc::new(SqliteSecretStore::new(db.clone(), wrapping.clone())),
        );
        let provider = providers
            .save_provider(CanvasProviderInput {
                name: "Toloveu".to_string(),
                protocol: "openai".to_string(),
                base_url: "https://toloveu.asia/v1".to_string(),
                enabled: true,
                models: vec![
                    model("gpt-5.6-sol", vec![CanvasScenario::Text]),
                    model("gpt-image-2", vec![CanvasScenario::ImageGeneration]),
                ],
                api_key: None,
                ..Default::default()
            })
            .expect("save provider");
        providers
            .set_provider_api_key(&provider.id, PLAINTEXT_KEY)
            .expect("set key");
        providers
            .save_bindings(
                None,
                vec![CanvasScenarioBinding {
                    scenario: "text".to_string(),
                    provider_id: provider.id.clone(),
                    model_id: "gpt-5.6-sol".to_string(),
                    enabled: true,
                }],
            )
            .expect("save bindings");

        let workflows = CanvasWorkflowStore::new(db.clone());
        workflows
            .write_current("creative", None, &graph(), None)
            .expect("write current");
        workflows
            .save_workflow(Some("wf-demo"), "演示工作流", "creative", None, &graph())
            .expect("save named");

        let artifacts =
            CanvasArtifactService::new(root.files_dir(), db.clone()).expect("artifacts");
        let imported = artifacts
            .import(&CanvasArtifactImport {
                kind: "image".to_string(),
                data_url: Some(png_data_url()),
                local_path: None,
                file_name: None,
                media_type: None,
                workspace_id: Some("ws-1".to_string()),
            })
            .expect("import artifact");

        let preferences = CanvasPreferencesStore::new(db.clone());
        preferences
            .write(
                "creative-library",
                &json!([{"id": "a1", "name": "参考图", "imageUrl": imported.uri}]),
            )
            .expect("write preferences");

        let bytes = artifacts
            .read_bytes(imported.id.as_str(), Some("ws-1"))
            .expect("read bytes")
            .expect("bytes present");
        (imported.uri, bytes)
    };

    // 第二次启动：只留下 TEMP 目录和系统钥匙串里的包裹键。
    let db = Arc::new(Database::open(root.db_path()).expect("reopen db"));
    let providers = CanvasProviderService::new(
        db.clone(),
        Arc::new(SqliteSecretStore::new(db.clone(), wrapping.clone())),
    );

    let workflows = CanvasWorkflowStore::new(db.clone());
    let restored = workflows
        .read_current("creative", None)
        .expect("read current")
        .graph
        .expect("current graph restored");
    assert_eq!(restored, graph());
    assert_eq!(
        restored["nodes"][0]["data"]["promptProfileVersion"],
        json!("creative.image-gen.v1")
    );
    let named = workflows
        .load_workflow("wf-demo", None)
        .expect("load named")
        .expect("named workflow restored");
    assert_eq!(named.name, "演示工作流");
    assert_eq!(named.graph, graph());

    let listed = providers.list_providers().expect("list providers");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].api_key_set, "key must still be marked present");
    assert!(
        !listed[0]
            .api_key_masked
            .as_deref()
            .unwrap_or_default()
            .contains(PLAINTEXT_KEY),
        "masked preview must not carry the key"
    );
    assert_eq!(
        providers
            .provider_api_key(&listed[0].id)
            .expect("read key")
            .as_deref(),
        Some(PLAINTEXT_KEY),
        "encrypted key must decrypt after restart"
    );
    let bindings = providers.bindings(None).expect("bindings");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].model_id, "gpt-5.6-sol");
    assert_eq!(bindings[0].provider_id, listed[0].id);

    let artifacts = CanvasArtifactService::new(root.files_dir(), db.clone()).expect("artifacts");
    let id = artifact_uri
        .strip_prefix("artifact://")
        .expect("artifact uri");
    let record = artifacts
        .record(id, Some("ws-1"))
        .expect("record")
        .expect("index row restored");
    assert_eq!(record.kind, ArtifactKind::Image);
    assert_eq!(record.media_type.as_deref(), Some("image/png"));
    assert_eq!(record.byte_size as usize, artifact_bytes.len());
    assert_eq!(
        artifacts
            .read_bytes(id, Some("ws-1"))
            .expect("read")
            .expect("bytes"),
        artifact_bytes
    );
    assert!(std::fs::metadata(&record.path)
        .expect("blob file")
        .is_file());
    assert_eq!(
        artifacts
            .list(Some("ws-1"), Some(ArtifactKind::Image))
            .expect("list")
            .len(),
        1
    );

    let preferences = CanvasPreferencesStore::new(db.clone());
    let library = preferences
        .read("creative-library")
        .expect("read preferences")
        .expect("preferences restored");
    assert_eq!(library[0]["imageUrl"], json!(artifact_uri));

    // 明文密钥不能出现在数据库文件的任何位置：只有密文落盘。
    let raw = std::fs::read(root.db_path()).expect("read db file");
    let as_text = String::from_utf8_lossy(&raw);
    assert!(
        !as_text.contains(PLAINTEXT_KEY),
        "plaintext api key leaked into the database file"
    );
}
