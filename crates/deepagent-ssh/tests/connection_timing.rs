use deepagent_ssh::{
    CreateSshConnectionRequest, SshConnectionConfig, SshService, UpdateSshConnectionRequest,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

fn timing() -> Value {
    json!({"connect_timeout_secs": 1, "inactivity_timeout_secs": 30,
        "keepalive_interval_secs": 5, "keepalive_max": 2})
}

fn request() -> Value {
    json!({"name":"timing fixture", "host":"127.0.0.1", "port":22,
        "username":"fixture", "auth_type":"password", "password":"fixture-only",
        "timing":timing()})
}

#[tokio::test]
async fn timing_round_trips_and_legacy_updates_preserve_it() {
    let root = tempfile::tempdir().unwrap();
    let service = SshService::new(root.path().to_owned());
    let created = service
        .create_connection(serde_json::from_value(request()).unwrap())
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&created).unwrap()["timing"], timing());
    let reloaded = SshService::new(root.path().to_owned());
    reloaded.load().await.unwrap();
    assert_eq!(
        serde_json::to_value(&reloaded.list_connections().await[0]).unwrap()["timing"],
        timing()
    );
    let mut update = request();
    update["id"] = json!(created.id);
    update.as_object_mut().unwrap().remove("timing");
    let saved = reloaded
        .update_connection(
            serde_json::from_value::<UpdateSshConnectionRequest>(update.clone()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(saved).unwrap()["timing"], timing());
    update["timing"] = json!({"connect_timeout_secs":20,"inactivity_timeout_secs":null,
        "keepalive_interval_secs":null,"keepalive_max":3});
    let saved = reloaded
        .update_connection(serde_json::from_value(update.clone()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(saved).unwrap()["timing"],
        update["timing"]
    );
}

#[test]
fn old_configs_receive_explicit_timing_defaults() {
    let mut old = request();
    old.as_object_mut().unwrap().remove("timing");
    old["id"] = json!("legacy");
    let config: SshConnectionConfig = serde_json::from_value(old).unwrap();
    assert_eq!(
        serde_json::to_value(config).unwrap()["timing"],
        json!({
        "connect_timeout_secs":20,"inactivity_timeout_secs":null,
        "keepalive_interval_secs":null,"keepalive_max":3})
    );
}

#[tokio::test]
async fn invalid_timing_is_rejected_before_persistence() {
    let root = tempfile::tempdir().unwrap();
    let service = SshService::new(root.path().to_owned());
    for (key, bad) in [
        ("connect_timeout_secs", 0),
        ("connect_timeout_secs", 86401),
        ("inactivity_timeout_secs", 0),
        ("inactivity_timeout_secs", 86401),
        ("keepalive_interval_secs", 0),
        ("keepalive_interval_secs", 86401),
        ("keepalive_max", 0),
        ("keepalive_max", 101),
    ] {
        let mut input = request();
        input["timing"][key] = json!(bad);
        let input: CreateSshConnectionRequest = serde_json::from_value(input).unwrap();
        assert!(
            service.create_connection(input).await.is_err(),
            "accepted {key}={bad}"
        );
        assert!(service.list_connections().await.is_empty());
    }
}

async fn stalled_handshake_uses_configured_timeout(entry: &'static str) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let root = tempfile::tempdir().unwrap();
    let trust = root.path().join("known_hosts");
    // This key is never sent: the fixture intentionally stalls before the SSH banner.
    let key =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    std::fs::write(
        &trust,
        format!(
            "[127.0.0.1]:{port} {}\n",
            key.public_key().to_openssh().unwrap()
        ),
    )
    .unwrap();
    let service = Arc::new(SshService::new(root.path().to_owned()));
    let mut input = request();
    input["port"] = json!(port);
    if entry == "begin_long" {
        input["timing"]["connect_timeout_secs"] = json!(300);
    }
    input["known_hosts_path"] = json!(trust);
    if entry.starts_with("begin") {
        input["known_hosts_path"] = json!(root.path().join("unknown_hosts"));
    }
    let dto = service
        .create_connection(serde_json::from_value(input.clone()).unwrap())
        .await
        .unwrap();
    let next = service.clone();
    let task = tokio::spawn(async move {
        match entry {
            "begin" | "begin_long" => next
                .begin_connection(&dto.id)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
            "test" => {
                let result = next.test_connection(&dto).await.unwrap();
                assert!(!result.ok);
                Err(result.error.unwrap())
            }
            _ => next
                .connect(&dto.id)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
        }
    });
    let (_stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(if entry == "begin_long" {
        121
    } else {
        1
    }))
    .await;
    let result = tokio::time::timeout(Duration::from_millis(100), task)
        .await
        .expect("configured one-second deadline was ignored")
        .unwrap();
    assert!(result.unwrap_err().contains(if entry == "begin_long" {
        "expired"
    } else {
        "1s"
    }));
}

#[tokio::test]
async fn saved_connect_has_a_configurable_network_deadline() {
    stalled_handshake_uses_configured_timeout("connect").await;
}

#[tokio::test]
async fn unknown_host_probe_has_a_configurable_network_deadline() {
    stalled_handshake_uses_configured_timeout("begin").await;
}

#[tokio::test]
async fn connection_test_has_a_configurable_network_deadline() {
    stalled_handshake_uses_configured_timeout("test").await;
}

#[tokio::test]
async fn long_network_timeout_cannot_extend_the_original_challenge_deadline() {
    stalled_handshake_uses_configured_timeout("begin_long").await;
}
