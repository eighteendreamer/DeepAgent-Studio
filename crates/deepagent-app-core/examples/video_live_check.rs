//! Explicit live diagnostics. Never run as part of the offline test suite.
//! `inspect` reads a SQLite snapshot; `install` installs the pinned media runtime;
//! `generate <provider::model>` submits ONE synthetic video (may incur charges).
use std::sync::Arc;

use deepagent_app_core::{
    canvas_artifact_service::CanvasArtifactService,
    canvas_model_gateway::CanvasModelGateway,
    canvas_provider_service::{CanvasProviderService, CanvasScenario},
    data_root::{self, UserPaths},
    runtime_service::{ReqwestDownloader, RuntimeService},
    KeychainStore, SqliteSecretStore, KEYCHAIN_SERVICE,
};
use deepagent_persistence::Database;
use deepagent_runtime::workflow::{CanvasModelBridge, CanvasVideoRequest};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("inspect");
    let paths = UserPaths::resolve()?;
    let runtime = RuntimeService::with_lookup_roots(
        std::slice::from_ref(&paths.runtimes),
        &[],
        Arc::new(ReqwestDownloader::default()),
    )
    .with_state_root(&paths.root);
    if mode == "install" {
        runtime.install("ffmpeg", Arc::new(|_, _| {})).await?;
        println!("ffmpeg={:?}", runtime.installed_executable("ffmpeg"));
        return Ok(());
    }
    if !matches!(mode, "inspect" | "generate" | "probe" | "catalog") {
        return Err("expected inspect, install, generate, probe or catalog <provider-id>".into());
    }
    println!("ffmpeg={:?}", runtime.installed_executable("ffmpeg"));
    // Database::open runs recovery; never open the live desktop DB that way.
    let snapshot = tempfile::tempdir()?;
    let db_path = snapshot.path().join("deepagent.db");
    let live = rusqlite::Connection::open_with_flags(
        data_root::resolve_database_path()?,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    live.execute("VACUUM INTO ?1", [db_path.to_string_lossy().as_ref()])?;
    drop(live);
    let db = Arc::new(Database::open(&db_path)?);
    let secrets = Arc::new(SqliteSecretStore::new(
        db.clone(),
        Arc::new(KeychainStore::new(KEYCHAIN_SERVICE)),
    ));
    let providers = Arc::new(CanvasProviderService::new(db, secrets));
    if mode == "inspect" {
        for provider in providers.list_providers()? {
            for model in provider.models.iter().filter(|model| {
                model
                    .scenarios
                    .iter()
                    .any(|value| value == "video_generation")
            }) {
                println!(
                    "provider={} protocol={} enabled={} key_present={} model={} model_enabled={}",
                    provider.id,
                    provider.protocol,
                    provider.enabled,
                    provider.api_key_set,
                    model.id,
                    model.enabled
                );
                if let Ok(url) = reqwest::Url::parse(&provider.base_url) {
                    println!(
                        "configured_host={} base_path={}",
                        url.host_str().unwrap_or(""),
                        url.path()
                    );
                }
            }
        }
        return Ok(());
    }
    let model_ref = args
        .get(2)
        .ok_or("generate requires an explicit provider::model")?;
    let output_root = paths.cache.join("video-live-check");
    let artifacts = Arc::new(CanvasArtifactService::new(
        &output_root,
        Arc::new(Database::open_in_memory()?),
    )?);
    let gateway = CanvasModelGateway::new(providers).with_artifacts(artifacts.clone());
    if mode == "catalog" {
        let provider_id = args.get(2).ok_or("catalog requires a provider id")?;
        let ids = gateway.discover_models(provider_id).await?;
        println!("model_catalog_count={}", ids.len());
        for id in ids {
            println!("model={id}");
        }
        return Ok(());
    }
    if mode == "probe" {
        let (provider_id, model_id) = model_ref
            .split_once("::")
            .ok_or("expected provider::model")?;
        let result = gateway
            .test_connection(provider_id, model_id, CanvasScenario::Text)
            .await;
        let status: String = result
            .message
            .split("HTTP ")
            .nth(1)
            .map(|tail| tail.chars().take_while(char::is_ascii_digit).collect())
            .unwrap_or_else(|| "none".into());
        println!(
            "chat_probe ok={} category={} http_status={} latency_ms={}",
            result.ok, result.code, status, result.latency_ms
        );
        return Ok(());
    }
    let output = gateway
        .generate_video(CanvasVideoRequest {
            model_ref: model_ref.clone(),
            prompt: "A blue sphere slowly rotating on a plain white background, static camera."
                .into(),
            image_url: None,
            size: None,
            resume_task_id: None,
            timeout_ms: 600_000,
            cancel: None,
            on_progress: None,
        })
        .await;
    match output {
        Ok(output) => {
            println!(
                "generated={}",
                artifacts.video_file_path(&output.artifact_uri)?.display()
            );
            Ok(())
        }
        Err(error) => {
            let detail = error.to_string();
            let status = detail
                .split("HTTP ")
                .nth(1)
                .map(|tail| {
                    tail.chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                })
                .unwrap_or_else(|| "none".into());
            let category = [
                "SecretMissing",
                "TransientFailure",
                "ProviderFailure",
                "UnsupportedCapability",
                "Cancelled",
                "Timeout",
            ]
            .into_iter()
            .find(|category| detail.contains(category))
            .unwrap_or("Other");
            eprintln!("video_request_failed category={category} http_status={status}");
            Err(
                "Video gateway request failed; response body suppressed to protect credentials"
                    .into(),
            )
        }
    }
}
