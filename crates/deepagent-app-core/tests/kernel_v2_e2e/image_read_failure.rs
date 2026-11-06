//! H-68: a retained image read failure is durable failure, never text fallback.
use super::*;
use deepagent_core::event::{Event, EventPayload};
use deepagent_core::response_item::ResponseItem;
use deepagent_persistence::{event_store::EventStore, run_store::RunStore};

#[derive(Clone, Copy, Debug)]
enum ReadFailure {
    Missing,
    Empty,
    Oversized,
    #[cfg(windows)]
    PermissionDenied,
}

pub(super) fn image_events(db: &Database, session: &str) -> Vec<Event> {
    EventStore::new(db)
        .load_session(session.parse().unwrap())
        .unwrap()
        .into_iter()
        .filter(|event| match &event.payload {
            EventPayload::MessageAppended { message } => !message.attachments.is_empty(),
            EventPayload::ResponseItemAppended {
                item: ResponseItem::InputMessage { attachments, .. },
            } => !attachments.is_empty(),
            _ => false,
        })
        .collect()
}

fn terminal(db: &Database, run: &str, kind: &str) {
    let events = RunStore::new(db).events_after(run, None).unwrap();
    let terminal: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].data["kind"], kind);
    assert_eq!(
        RunStore::new(db)
            .get(run)
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some(kind)
    );
}

async fn observe_failure_and_recovery(failure: ReadFailure) {
    let root = tempfile::tempdir().unwrap();
    let dto = AttachmentService::new(root.path().join("attachments"))
        .ingest(AttachmentIngestDto {
            id: Some("h68-retained".into()), session_id: None, kind: "image".into(),
            name: "pixel.png".into(), mime: "image/png".into(), source: "picker".into(),
            local_path: None, text: None,
            data_url: Some(concat!("data:image/png;base64,",
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=").into()),
        }).unwrap();
    let path = PathBuf::from(dto.original_path.as_ref().unwrap());
    let original = std::fs::read(&path).unwrap();
    let counts = Arc::new(Mutex::new(Vec::new()));
    let hashes = Arc::new(Mutex::new(Vec::new()));
    let transport = Arc::new(ImageAnswer {
        images_per_request: counts.clone(),
        request_bytes: None,
        image_hashes: Some(hashes.clone()),
    });
    let database = root.path().join("history.sqlite");
    let session;
    let admitted;
    {
        let db = Arc::new(Database::open(&database).unwrap());
        let chat = ChatService::new(
            db.clone(),
            image_settings(db.clone()).await,
            transport.clone(),
            root.path(),
        );
        session = chat
            .run_in_session_with_attachments(
                "Inspect the synthetic image",
                None,
                None,
                None,
                vec![],
                None,
                false,
                Some("h68-first"),
                vec![InputAttachment::Image {
                    id: dto.id.clone(),
                    path: path.clone(),
                    media_type: dto.mime.clone(),
                    sha256: dto.sha256.clone(),
                }],
                |_| {},
                |_| {},
            )
            .await
            .unwrap();
        admitted = image_events(&db, &session);
        assert_eq!(
            admitted.len(),
            2,
            "both UI and native durable references must exist"
        );
        terminal(&db, "h68-first", "succeeded");
    }
    #[cfg(windows)]
    let mut denied = None;
    let reason = match failure {
        ReadFailure::Missing => {
            std::fs::remove_file(&path).unwrap();
            "open image failed"
        }
        ReadFailure::Empty => {
            std::fs::write(&path, []).unwrap();
            "nonempty regular file within the byte limit"
        }
        ReadFailure::Oversized => {
            std::fs::File::create(&path)
                .unwrap()
                .set_len(32 * 1024 * 1024 + 1)
                .unwrap();
            "nonempty regular file within the byte limit"
        }
        #[cfg(windows)]
        ReadFailure::PermissionDenied => {
            denied = Some(DeniedImage::new(&path));
            assert_eq!(
                std::fs::File::open(&path).unwrap_err().raw_os_error(),
                Some(5)
            );
            "open image failed"
        }
    };
    {
        let db = Arc::new(Database::open(&database).unwrap());
        let chat = ChatService::new(
            db.clone(),
            image_settings(db.clone()).await,
            transport.clone(),
            root.path(),
        );
        let error = chat
            .run_in_session(
                "Refer to the retained image",
                Some(&session),
                None,
                None,
                vec![],
                None,
                false,
                Some("h68-unreadable"),
                |_| {},
                |_| {},
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("attachment_integrity") && error.contains(reason),
            "{error}"
        );
        assert_eq!(
            *counts.lock().unwrap(),
            vec![1],
            "failed continuation must not reach transport"
        );
        terminal(&db, "h68-unreadable", "failed");
        assert_eq!(
            image_events(&db, &session),
            admitted,
            "never remove or resign prior events"
        );
    }
    // A second real database reopen verifies the failed run and references,
    // then restores exactly the admitted bytes without reimporting or editing history.
    #[cfg(windows)]
    if let Some(denied) = denied.take() {
        drop(denied);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
    std::fs::write(&path, &original).unwrap();
    let db = Arc::new(Database::open(&database).unwrap());
    terminal(&db, "h68-unreadable", "failed");
    assert_eq!(image_events(&db, &session), admitted);
    let chat = ChatService::new(
        db.clone(),
        image_settings(db.clone()).await,
        transport,
        root.path(),
    );
    chat.run_in_session(
        "The original synthetic image is restored",
        Some(&session),
        None,
        None,
        vec![],
        None,
        false,
        Some("h68-recovered"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    terminal(&db, "h68-recovered", "succeeded");
    assert_eq!(*counts.lock().unwrap(), vec![1, 1]);
    assert_eq!(
        *hashes.lock().unwrap(),
        vec![vec![dto.sha256.clone().unwrap()]; 2]
    );
    assert_eq!(image_events(&db, &session), admitted);
    eprintln!("H68 {failure:?}: failed_before_transport=true terminal_once=true references_unchanged=true reopen_recovered=true wire_counts=[1,1]");
}

#[tokio::test]
async fn unreadable_image_failure_and_original_byte_recovery_survive_sqlite_reopen() {
    for failure in [
        ReadFailure::Missing,
        ReadFailure::Empty,
        ReadFailure::Oversized,
    ] {
        observe_failure_and_recovery(failure).await;
    }
}

#[cfg(windows)]
fn acl(path: &std::path::Path, action: &str, restore: &str) -> String {
    // Reuse the established H229/file_metadata fixture API. Deny only ReadData
    // on this TempDir file; retain ReadControl/WriteDac for exact restoration.
    let script = r#"
$ErrorActionPreference = 'Stop'
$path = $env:DEEPAGENT_H68_ACL_PATH
$section = [System.Security.AccessControl.AccessControlSections]::Access
$policy = [System.IO.File]::GetAccessControl($path)
$original = $policy.GetSecurityDescriptorSddlForm($section)
if ($env:DEEPAGENT_H68_ACL_ACTION -eq 'restore') {
    $policy.SetSecurityDescriptorSddlForm($env:DEEPAGENT_H68_ACL_RESTORE, $section)
    [System.IO.File]::SetAccessControl($path, $policy)
} elseif ($env:DEEPAGENT_H68_ACL_ACTION -eq 'deny') {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
        $identity, [System.Security.AccessControl.FileSystemRights]::ReadData,
        [System.Security.AccessControl.AccessControlType]::Deny)
    $policy.AddAccessRule($rule)
    [System.IO.File]::SetAccessControl($path, $policy)
}
[Console]::Write($original)
"#;
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("DEEPAGENT_H68_ACL_PATH", path)
        .env("DEEPAGENT_H68_ACL_ACTION", action)
        .env("DEEPAGENT_H68_ACL_RESTORE", restore)
        .output()
        .expect("local PowerShell required for the synthetic ACL fixture");
    assert!(
        output.status.success(),
        "synthetic image ACL operation failed"
    );
    String::from_utf8(output.stdout).unwrap()
}

#[cfg(windows)]
struct DeniedImage(PathBuf, String);

#[cfg(windows)]
impl DeniedImage {
    fn new(path: &std::path::Path) -> Self {
        Self(path.to_path_buf(), acl(path, "deny", ""))
    }
}

#[cfg(windows)]
impl Drop for DeniedImage {
    fn drop(&mut self) {
        acl(&self.0, "restore", &self.1);
        assert!(
            acl(&self.0, "capture", "") == self.1,
            "original DACL must be restored exactly"
        );
    }
}

#[cfg(windows)]
#[tokio::test]
async fn real_read_data_dacl_denial_fails_without_sampling_and_recovers() {
    observe_failure_and_recovery(ReadFailure::PermissionDenied).await;
}
