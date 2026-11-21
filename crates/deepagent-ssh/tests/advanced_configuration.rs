use deepagent_ssh::{CreateSshConnectionRequest, SshService, UpdateSshConnectionRequest};
use serde_json::{json, Value};

fn request() -> Value {
    json!({"name":"advanced", "host":"fixture.invalid", "port":22,
        "username":"fixture", "auth_type":"agent"})
}

#[tokio::test]
async fn legacy_connections_disable_forwarding_and_compatibility() {
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().into());
    let dto = service
        .create_connection(serde_json::from_value(request()).unwrap())
        .await
        .unwrap();
    let wire = serde_json::to_value(dto).unwrap();
    assert_eq!(wire["advanced"]["forward_agent"], false);
    assert_eq!(wire["advanced"]["allow_legacy_algorithms"], false);
    assert!(wire["advanced"]["x11"].is_null());
}

#[tokio::test]
async fn advanced_options_roundtrip_and_legacy_update_preserves_them() {
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().into());
    let mut value = request();
    value["advanced"] = json!({"forward_agent":true,"agent_provider":"open_ssh",
        "allow_legacy_algorithms":true,"x11":{"display":"127.0.0.1:0","xauthority_path":null}});
    let saved = service
        .create_connection(
            serde_json::from_value::<CreateSshConnectionRequest>(value.clone()).unwrap(),
        )
        .await
        .unwrap();
    let expected = value["advanced"].clone();
    value.as_object_mut().unwrap().remove("advanced");
    value["id"] = json!(saved.id);
    let updated = service
        .update_connection(serde_json::from_value::<UpdateSshConnectionRequest>(value).unwrap())
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(updated).unwrap()["advanced"], expected);
    let reopened = SshService::new(data.path().into());
    reopened.load().await.unwrap();
    assert_eq!(
        serde_json::to_value(&reopened.list_connections().await[0]).unwrap()["advanced"],
        expected
    );
}

#[tokio::test]
async fn x11_rejects_remote_displays_and_malformed_options_before_persisting() {
    let data = tempfile::tempdir().unwrap();
    let service = SshService::new(data.path().into());
    for display in [
        "evil.invalid:0",
        "127.0.0.1:-1",
        "127.0.0.1:65535",
        "",
        "127.0.0.1:0\n",
    ] {
        let mut value = request();
        value["advanced"] = json!({"x11":{"display":display}});
        let parsed = serde_json::from_value::<CreateSshConnectionRequest>(value);
        if let Ok(parsed) = parsed {
            assert!(
                service.create_connection(parsed).await.is_err(),
                "{display:?}"
            );
        }
    }
    assert!(service.list_connections().await.is_empty());
}
