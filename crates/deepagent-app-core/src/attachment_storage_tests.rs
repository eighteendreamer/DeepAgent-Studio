//! H-58: real disposable attachment ownership fixtures, never user storage.

use super::*;

fn input(id: &str, session: Option<&str>) -> AttachmentIngestDto {
    AttachmentIngestDto {
        id: Some(id.into()),
        session_id: session.map(str::to_owned),
        kind: "text".into(),
        name: "canary.txt".into(),
        mime: "text/plain".into(),
        source: "paste".into(),
        local_path: None,
        data_url: None,
        text: Some("new content".into()),
    }
}

#[test]
fn invalid_identities_are_errors_before_storage_creation_and_never_alias() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("attachments");
    let service = AttachmentService::new(root.clone());
    for value in [
        "",
        ".",
        "..",
        "...",
        "a.",
        "a ",
        "a/b",
        r"a\b",
        "a:b",
        "C:",
        r"C:\absolute",
        r"\\host\share",
        r"\\?\C:\path",
        "a\0b",
        "a\nb",
        "你好",
        "a?b",
        "CON",
        "con.txt",
        "NUL",
        "AUX",
        "PRN",
        "COM1",
        "LPT9.txt",
        "CONIN$",
        "COM¹",
    ] {
        assert!(
            service.ingest(input(value, Some("session"))).is_err(),
            "id={value:?}"
        );
        assert!(
            service.ingest(input("first", Some(value))).is_err(),
            "session={value:?}"
        );
        assert!(service.remove(Some("session"), value).is_err());
        assert!(service.remove(Some(value), "first").is_err());
        assert!(
            !root.exists(),
            "invalid identity must not create storage: {value:?}"
        );
    }
    for (id, bucket, bad_id, bad_bucket) in [
        ("a_b", "session", "a/b", "session"),
        ("first", "session_a", "first", "session/a"),
    ] {
        let valid = service.ingest(input(id, Some(bucket))).unwrap();
        let original = valid.original_path.unwrap();
        let metadata = fs::read(Path::new(&valid.storage_dir).join("metadata.json")).unwrap();
        assert!(service.ingest(input(bad_id, Some(bad_bucket))).is_err());
        assert!(service.remove(Some(bad_bucket), bad_id).is_err());
        assert_eq!(fs::read_to_string(original).unwrap(), "new content");
        assert_eq!(
            fs::read(Path::new(&valid.storage_dir).join("metadata.json")).unwrap(),
            metadata
        );
    }
}

#[test]
fn valid_pending_generated_identity_and_reopened_removal_keep_dto_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("附件");
    let service = AttachmentService::new(root.clone());
    let mut generated = input("unused", None);
    generated.id = None;
    let dto = service.ingest(generated).unwrap();
    assert!(dto.id.starts_with("att_"));
    assert_eq!(dto.session_id, None);
    assert_eq!(
        Path::new(&dto.storage_dir),
        root.join("pending").join(&dto.id)
    );
    assert_eq!(
        dto.original_path.as_deref(),
        root.join("pending")
            .join(&dto.id)
            .join("canary.txt")
            .to_str()
    );
    let with_dots = service
        .ingest(input("att.v1-2_3", Some("session.v1")))
        .unwrap();
    assert_eq!(with_dots.id, "att.v1-2_3");
    assert_eq!(with_dots.session_id.as_deref(), Some("session.v1"));
    let reopened = AttachmentService::new(root.clone());
    assert!(reopened.remove(None, &dto.id).unwrap());
    assert!(!reopened.remove(None, &dto.id).unwrap());
    assert!(!reopened
        .remove(Some("missing-session"), "missing-id")
        .unwrap());
    assert!(Path::new(&with_dots.storage_dir)
        .join("metadata.json")
        .exists());
    assert!(root.join("pending").is_dir());
}

#[test]
fn local_import_and_display_filename_cleaning_remain_supported() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.txt");
    fs::write(&source, "local fixture").unwrap();
    let service = AttachmentService::new(temp.path().join("attachments"));
    let mut request = input("first", None);
    request.text = None;
    request.local_path = Some(source.to_string_lossy().into());
    request.name = "my secret 你好.txt".into();
    let dto = service.ingest(request).unwrap();
    assert_eq!(dto.name, "my_secret___.txt");
    assert_eq!(dto.extracted_text.as_deref(), Some("local fixture"));
    assert_eq!(dto.size_bytes, 13);
    assert_eq!(fs::read_to_string(source).unwrap(), "local fixture");
    let metadata: AttachmentDto = serde_json::from_slice(
        &fs::read(Path::new(&dto.storage_dir).join("metadata.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(metadata, dto);
}

#[test]
fn publication_does_not_truncate_hardlinked_original_extracted_or_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("attachments");
    let folder = root.join("session/first");
    fs::create_dir_all(&folder).unwrap();
    for name in ["canary.txt", "extracted.txt", "metadata.json"] {
        let outside = temp.path().join(name);
        fs::write(&outside, "outside canary").unwrap();
        fs::hard_link(&outside, folder.join(name)).unwrap();
    }
    let dto = AttachmentService::new(root)
        .ingest(input("first", Some("session")))
        .unwrap();
    for name in ["canary.txt", "extracted.txt", "metadata.json"] {
        assert_eq!(
            fs::read_to_string(temp.path().join(name)).unwrap(),
            "outside canary"
        );
    }
    assert_eq!(
        fs::read_to_string(folder.join("canary.txt")).unwrap(),
        "new content"
    );
    assert_eq!(
        fs::read_to_string(folder.join("extracted.txt")).unwrap(),
        "new content"
    );
    let metadata: AttachmentDto =
        serde_json::from_slice(&fs::read(folder.join("metadata.json")).unwrap()).unwrap();
    assert_eq!(metadata, dto);
}

#[test]
fn exact_removal_keeps_same_id_in_other_session_and_refuses_a_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("attachments");
    let service = AttachmentService::new(root.clone());
    assert!(!service.remove(None, "missing").unwrap());
    assert!(!root.exists());
    let first = service
        .ingest(input("same-id", Some("first-session")))
        .unwrap();
    let second = service
        .ingest(input("same-id", Some("second-session")))
        .unwrap();
    assert!(service.remove(Some("first-session"), "same-id").unwrap());
    assert!(!Path::new(&first.storage_dir).exists());
    assert!(Path::new(&second.storage_dir)
        .join("metadata.json")
        .exists());
    let file = root.join("second-session/not-a-directory");
    fs::write(&file, "file canary").unwrap();
    assert!(service
        .remove(Some("second-session"), "not-a-directory")
        .is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "file canary");
}

#[test]
fn replaced_root_and_initial_ancestor_are_not_new_storage_authority() {
    for missing_root in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("owner");
        let root = parent.join("attachments");
        fs::create_dir_all(if missing_root { &parent } else { &root }).unwrap();
        let service = AttachmentService::new(root.clone());
        let clone = service.clone();
        let replaced = if missing_root { &parent } else { &root };
        fs::rename(replaced, temp.path().join("original-owner")).unwrap();
        fs::create_dir_all(root.join("session/first")).unwrap();
        let canary = root.join("session/first/canary.txt");
        fs::write(&canary, "replacement canary").unwrap();
        assert!(service.ingest(input("first", Some("session"))).is_err());
        assert!(clone.remove(Some("session"), "first").is_err());
        assert_eq!(fs::read_to_string(canary).unwrap(), "replacement canary");
        assert!(!root.join("session/first/metadata.json").exists());
    }
}

#[cfg(any(unix, windows))]
fn directory_link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link.to_string_lossy().replace('/', "\\"))
            .arg(target.to_string_lossy().replace('/', "\\"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}

#[test]
#[cfg(any(unix, windows))]
fn linked_bucket_attachment_and_output_are_not_storage_identities() {
    for component in ["bucket", "attachment", "original", "extracted", "metadata"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("attachments");
        let outside = temp.path().join("outside");
        fs::create_dir_all(root.join("session/first")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("canary.txt"), "outside canary").unwrap();
        let link = match component {
            "bucket" => root.join("session"),
            "attachment" => root.join("session/first"),
            "original" => root.join("session/first/canary.txt"),
            "extracted" => root.join("session/first/extracted.txt"),
            _ => root.join("session/first/metadata.json"),
        };
        if link.is_dir() {
            // Canonicalize the exact target before deleting only our fixture.
            assert!(link
                .canonicalize()
                .unwrap()
                .starts_with(root.canonicalize().unwrap()));
            fs::remove_dir_all(&link).unwrap();
        }
        directory_link(&outside, &link);
        let service = AttachmentService::new(root.clone());
        assert!(
            service.ingest(input("first", Some("session"))).is_err(),
            "{component}"
        );
        if matches!(component, "bucket" | "attachment") {
            assert!(service.remove(Some("session"), "first").is_err());
        } else {
            // Recursive cleanup may unlink child links but must never follow.
            assert!(service.remove(Some("session"), "first").unwrap());
        }
        assert_eq!(
            fs::read_to_string(outside.join("canary.txt")).unwrap(),
            "outside canary"
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
        if fs::symlink_metadata(&link).is_ok() {
            #[cfg(unix)]
            fs::remove_file(&link).unwrap();
            #[cfg(windows)]
            fs::remove_dir(&link).unwrap();
        }
    }
}
