use super::*;
use crate::file_cache::FileStateCache;
use crate::file_tools::{EditFileTool, MultiEditTool, ReadFileTool, WriteFileTool};
use deepagent_tools::{Tool, ToolObservationScope};
use serde_json::json;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const INITIAL: &str = "owner=initial\nvalue=old\n";
const EXTERNAL: &str = "owner=external\nvalue=old\n";

struct Files {
    _dir: tempfile::TempDir,
    root: WorkspaceRoot,
    cache: SharedFileStateCache,
}

impl Files {
    fn new(content: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shared.txt"), content).unwrap();
        Self {
            root: WorkspaceRoot::new(dir.path()),
            _dir: dir,
            cache: Arc::new(Mutex::new(FileStateCache::new())),
        }
    }

    fn path(&self) -> std::path::PathBuf {
        self.root.path().join("shared.txt")
    }
    fn disk(&self) -> String {
        std::fs::read_to_string(self.path()).unwrap()
    }
    fn reader(&self) -> ReadFileTool {
        ReadFileTool::with_cache(self.root.clone(), self.cache.clone())
    }

    async fn read(&self) {
        let result = self
            .reader()
            .invoke(json!({"path":"shared.txt"}))
            .await
            .unwrap();
        assert!(result.ok, "{result:?}");
    }

    async fn change(&self, tool: usize, owner: &str) -> ToolOutput {
        self.change_in(tool, owner, ToolExecutionContext::default())
            .await
    }

    async fn change_in(
        &self,
        tool: usize,
        owner: &str,
        context: ToolExecutionContext,
    ) -> ToolOutput {
        match tool {
            0 => WriteFileTool::with_cache(self.root.clone(), self.cache.clone())
                .invoke_with_context(
                    json!({"path":"shared.txt", "content":owner.replace("value=old", "value=new")}),
                    context,
                )
                .await
                .unwrap(),
            1 => EditFileTool::with_cache(self.root.clone(), self.cache.clone())
                .invoke_with_context(
                    json!({"path":"shared.txt", "old":"value=old", "new":"value=new"}),
                    context,
                )
                .await
                .unwrap(),
            _ => MultiEditTool::with_cache(self.root.clone(), self.cache.clone())
                .invoke_with_context(
                    json!({"path":"shared.txt", "edits":[{"old":"value=old", "new":"value=new"}]}),
                    context,
                )
                .await
                .unwrap(),
        }
    }
}

fn scoped(scope: &ToolObservationScope) -> ToolExecutionContext {
    ToolExecutionContext::default().with_observation(scope.capture())
}

async fn read_in(files: &Files, scope: &ToolObservationScope) -> ToolOutput {
    files
        .reader()
        .invoke_with_context(json!({"path":"shared.txt"}), scoped(scope))
        .await
        .unwrap()
}

fn no_staging(root: &Path) {
    assert!(!std::fs::read_dir(root).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".deepagent-write-")));
}

#[tokio::test]
async fn context_reset_revokes_all_mutations_and_forces_a_real_reread() {
    for tool in 0..3 {
        let files = Files::new(INITIAL);
        let other = Files::new(INITIAL);
        let scope = ToolObservationScope::default();
        assert!(read_in(&files, &scope).await.ok);
        other.read().await;
        let stub = read_in(&files, &scope).await;
        assert!(stub.value.to_string().contains("FILE_UNCHANGED_STUB"));
        let old_invocation = scoped(&scope);
        scope.reset();
        assert!(!files.change_in(tool, INITIAL, old_invocation).await.ok);
        let rejected = files.change_in(tool, INITIAL, scoped(&scope)).await;
        assert!(
            !rejected.ok,
            "mutation {tool} kept discarded read authority"
        );
        assert_eq!(files.disk(), INITIAL);
        no_staging(files.root.path());
        // Another actor/cache is unaffected; reset is not a global permission flag.
        assert!(other.change(tool, INITIAL).await.ok);
        let reread = read_in(&files, &scope).await;
        assert!(reread.ok);
        assert_eq!(reread.value["cache_hit"], true);
        assert!(reread.value.to_string().contains("owner=initial"));
        assert!(!reread.value.to_string().contains("FILE_UNCHANGED_STUB"));
        assert!(files.change_in(tool, INITIAL, scoped(&scope)).await.ok);
        assert!(files.disk().contains("value=new"));
    }
}

#[tokio::test]
async fn resetting_a_shared_cache_sibling_preserves_parent_authority() {
    for tool in 0..3 {
        let files = Files::new(INITIAL);
        let parent = ToolObservationScope::default();
        let hook = ToolObservationScope::default();
        assert!(read_in(&files, &parent).await.ok);
        let nested = read_in(&files, &hook).await;
        assert_eq!(nested.value["cache_hit"], true);
        assert!(nested.value["content"]
            .as_str()
            .unwrap()
            .contains("owner=initial"));
        hook.reset();
        assert!(!files.change_in(tool, INITIAL, scoped(&hook)).await.ok);
        assert_eq!(read_in(&files, &parent).await.value["unchanged_stub"], true);
        assert!(files.change_in(tool, INITIAL, scoped(&parent)).await.ok);
        assert!(files.disk().contains("value=new"));
    }
}

#[tokio::test]
async fn shared_bytes_do_not_share_windows_or_legacy_authority() {
    let files = Files::new(INITIAL);
    let parent = ToolObservationScope::default();
    let hook = ToolObservationScope::default();
    let read = files.reader();
    let first = read
        .invoke_with_context(json!({"path":"shared.txt","limit":1}), scoped(&parent))
        .await
        .unwrap();
    let second = read
        .invoke_with_context(json!({"path":"shared.txt","offset":2}), scoped(&hook))
        .await
        .unwrap();
    assert!(first.ok && second.ok);
    assert_eq!(second.value["cache_hit"], true);
    assert!(!files.change_in(0, INITIAL, scoped(&parent)).await.ok);
    assert!(!files.change_in(0, INITIAL, scoped(&hook)).await.ok);
    assert!(!files.change_in(1, INITIAL, scoped(&parent)).await.ok);
    assert!(
        !files.change(1, INITIAL).await.ok,
        "unscoped cannot inherit scoped reads"
    );
    files.read().await;
    let fresh = ToolObservationScope::default();
    assert!(
        !files.change_in(1, INITIAL, scoped(&fresh)).await.ok,
        "scoped cannot inherit legacy reads"
    );
    assert!(files.change_in(1, INITIAL, scoped(&hook)).await.ok);
    assert!(files.disk().contains("value=new"));
}

#[tokio::test]
async fn late_old_read_cannot_authorize_the_replacement_context() {
    let files = Files::new(INITIAL);
    let scope = ToolObservationScope::default();
    let old_context = scoped(&scope);
    let gate = lock_path_operation(&files.path()).unwrap();
    let reader = files.reader();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let pending = tokio::spawn(async move {
        started.send(()).unwrap();
        reader
            .invoke_with_context(json!({"path":"shared.txt"}), old_context)
            .await
            .unwrap()
    });
    waiting.await.unwrap();
    scope.reset();
    drop(gate);
    assert!(pending.await.unwrap().ok);
    assert!(!files.change_in(0, INITIAL, scoped(&scope)).await.ok);
    assert_eq!(files.disk(), INITIAL);
    let reread = read_in(&files, &scope).await;
    assert_eq!(reread.value["cache_hit"], true);
    assert!(reread.value["content"]
        .as_str()
        .unwrap()
        .contains("owner=initial"));
    assert!(files.change_in(0, INITIAL, scoped(&scope)).await.ok);
}

#[cfg(windows)]
mod access_policy {
    use super::*;
    use deepagent_persistence::fs_commit::access::WindowsDacl;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{READ_CONTROL, WRITE_DAC};

    fn policy(sddl: &str) -> WindowsDacl {
        serde_json::from_value(json!({"version":1,"protected":true,"sddl":sddl})).unwrap()
    }

    fn set(path: &Path, policy: &WindowsDacl) {
        let file = std::fs::OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .open(path)
            .unwrap();
        policy.apply(&file, false).unwrap();
    }

    fn capture(path: &Path) -> WindowsDacl {
        WindowsDacl::capture(&std::fs::File::open(path).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn all_content_tools_preserve_explicit_allow_deny_and_inherited_policies() {
        for explicit in [false, true] {
            for tool in 0..3 {
                let files = Files::new(INITIAL);
                if explicit {
                    set(&files.path(), &policy("D:P(D;;0x20;;;WD)(A;;FA;;;WD)"));
                }
                let before = capture(&files.path());
                files.read().await;
                let result = files.change(tool, INITIAL).await;
                assert!(result.ok, "tool={tool}, explicit={explicit}: {result:?}");
                let file = std::fs::File::open(files.path()).unwrap();
                assert!(
                    before
                        .verify_publication(&file, || {
                            deepagent_persistence::fs_commit::access::open_policy_source(
                                files.root.path(),
                            )
                        })
                        .unwrap(),
                    "tool changed DACL"
                );
                assert_eq!(files.disk(), INITIAL.replace("value=old", "value=new"));
                no_staging(files.root.path());
            }
        }
    }

    #[tokio::test]
    async fn first_creation_keeps_default_parent_inheritance() {
        let files = Files::new(INITIAL);
        let expected = capture(&files.path());
        let result = WriteFileTool::with_cache(files.root.clone(), files.cache.clone())
            .invoke(json!({"path":"created.txt","content":"synthetic first write"}))
            .await
            .unwrap();
        assert!(result.ok, "{result:?}");
        assert!(capture(&files.root.path().join("created.txt")) == expected);
        no_staging(files.root.path());
    }

    #[test]
    fn inherited_stage_policy_is_verified_before_publication() {
        use deepagent_persistence::fs_commit::access::open_policy_source;
        let files = Files::new(INITIAL);
        let before = capture(&files.path());
        let target = files.root.write_target(&files.path(), false).unwrap();
        let mut stage = target.stage().unwrap();
        use std::io::Write;
        stage
            .file_mut()
            .write_all(b"published synthetic body")
            .unwrap();
        stage.publish(&target, true).unwrap();
        assert_eq!(files.disk(), "published synthetic body");
        assert!(before
            .verify_publication(&open_policy_source(&files.path()).unwrap(), || {
                open_policy_source(files.root.path())
            })
            .unwrap());
        no_staging(files.root.path());
    }

    #[test]
    fn empty_stage_is_protected_and_policy_change_rejects_publication() {
        use std::io::Write;
        let files = Files::new(INITIAL);
        let before = policy("D:P(D;;0x20;;;WD)(A;;FA;;;WD)");
        set(&files.path(), &before);
        let before = capture(&files.path());
        let target = files.root.write_target(&files.path(), false).unwrap();
        let mut stage = target.stage().unwrap();
        assert_eq!(stage.file_mut().metadata().unwrap().len(), 0);
        assert!(
            WindowsDacl::capture(stage.file_mut()).unwrap() == before,
            "unprotected empty staging entry"
        );
        stage
            .file_mut()
            .write_all(b"must not be published")
            .unwrap();
        let changed = policy("D:P(A;;FA;;;WD)");
        set(&files.path(), &changed);
        let changed = capture(&files.path());
        let error = stage.publish(&target, true).unwrap_err();
        assert!(error.to_string().contains("FILE_ACCESS_POLICY_CHANGED"));
        assert_eq!(files.disk(), INITIAL);
        assert!(
            capture(&files.path()) == changed,
            "failed write reverted external DACL change"
        );
        no_staging(files.root.path());
    }
}

#[tokio::test]
async fn every_mutation_preserves_external_edits_until_fresh_read() {
    for tool in 0..3 {
        let files = Files::new(INITIAL);
        files.read().await;
        std::fs::write(files.path(), EXTERNAL).unwrap();
        let stale = files.change(tool, INITIAL).await;
        assert!(!stale.ok, "tool={tool}: {stale:?}");
        assert!(stale.value["error"]
            .as_str()
            .unwrap()
            .contains("FILE_CHANGED_SINCE_READ"));
        assert_eq!(files.disk(), EXTERNAL);
        files.read().await;
        assert!(files.change(tool, EXTERNAL).await.ok);
        assert_eq!(files.disk(), EXTERNAL.replace("value=old", "value=new"));
        no_staging(files.root.path());
    }
}

#[tokio::test]
async fn identical_metadata_cannot_hide_different_content_or_return_a_stale_stub() {
    const SAME_LENGTH: &str = "owner=changed\nvalue=old\n";
    for tool in 0..3 {
        let files = Files::new(INITIAL);
        files.read().await;
        let previous = std::fs::metadata(files.path()).unwrap();
        assert_eq!(INITIAL.len(), SAME_LENGTH.len());
        std::fs::write(files.path(), SAME_LENGTH).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(files.path())
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(previous.modified().unwrap()))
            .unwrap();
        let current = std::fs::metadata(files.path()).unwrap();
        assert_eq!(current.len(), previous.len());
        assert_eq!(current.modified().unwrap(), previous.modified().unwrap());
        assert!(!files.change(tool, INITIAL).await.ok);
        assert_eq!(files.disk(), SAME_LENGTH);
        let read = files
            .reader()
            .invoke(json!({"path":"shared.txt"}))
            .await
            .unwrap();
        assert!(read.ok);
        assert_ne!(read.value["unchanged_stub"], true);
        assert!(read.value["content"]
            .as_str()
            .unwrap()
            .contains("owner=changed"));
        assert!(files.change(tool, SAME_LENGTH).await.ok);
        assert_eq!(files.disk(), SAME_LENGTH.replace("value=old", "value=new"));
    }
}

#[tokio::test]
async fn timestamp_changes_alone_do_not_revoke_an_identical_observed_version() {
    let files = Files::new(INITIAL);
    files.read().await;
    let time = std::fs::metadata(files.path()).unwrap().modified().unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(files.path())
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(time + Duration::from_secs(2)))
        .unwrap();
    assert!(files.change(0, INITIAL).await.ok);
    assert_eq!(files.disk(), INITIAL.replace("value=old", "value=new"));
}

#[cfg(windows)]
#[tokio::test]
async fn native_publication_failure_preserves_version_and_allows_retry_without_reread() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    let files = Files::new(INITIAL);
    files.read().await;
    let owner = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(files.path())
        .unwrap();
    let result = files.change(0, INITIAL).await;
    assert!(!result.ok, "{result:?}");
    assert!(result.value["error"]
        .as_str()
        .unwrap()
        .contains("write failed"));
    assert_eq!(files.disk(), INITIAL);
    no_staging(files.root.path());
    drop(owner);
    assert!(files.change(0, INITIAL).await.ok);
    assert_eq!(files.disk(), INITIAL.replace("value=old", "value=new"));
}

#[tokio::test]
async fn deleted_observed_file_is_not_silently_recreated() {
    for tool in 0..3 {
        let files = Files::new(INITIAL);
        files.read().await;
        std::fs::remove_file(files.path()).unwrap();
        let result = files.change(tool, INITIAL).await;
        assert!(!result.ok);
        assert!(result.value["error"]
            .as_str()
            .unwrap()
            .contains("FILE_CHANGED_SINCE_READ"));
        assert!(!files.path().exists());
    }
}

#[tokio::test]
async fn cached_bytes_without_delivered_window_cannot_authorize_writes() {
    let files = Files::new(INITIAL);
    files.cache.lock().unwrap().store(
        files.path(),
        INITIAL.into(),
        &std::fs::metadata(files.path()).unwrap(),
    );
    assert!(!files.change(0, INITIAL).await.ok);
    assert_eq!(files.disk(), INITIAL);
}

#[tokio::test]
async fn partial_reads_authorize_only_the_delivered_ranges() {
    let files = Files::new("visible\nhidden\ntail\n");
    let read = files
        .reader()
        .invoke(json!({"path":"shared.txt", "limit":1}))
        .await
        .unwrap();
    assert_eq!(read.value["truncated"], true);
    let stub = files
        .reader()
        .invoke(json!({"path":"shared.txt", "limit":1}))
        .await
        .unwrap();
    assert_eq!(stub.value["unchanged_stub"], true);
    assert_eq!(stub.value["truncated"], true);
    assert_eq!(stub.value["next_offset"], 2);
    let write = WriteFileTool::with_cache(files.root.clone(), files.cache.clone());
    let result = write
        .invoke(json!({"path":"shared.txt", "content":"replacement"}))
        .await
        .unwrap();
    assert!(!result.ok);
    assert!(result.value["error"]
        .as_str()
        .unwrap()
        .contains("FILE_RANGE_NOT_READ"));
    let edit = EditFileTool::with_cache(files.root.clone(), files.cache.clone());
    assert!(
        !edit
            .invoke(json!({"path":"shared.txt", "old":"hidden", "new":"lost"}))
            .await
            .unwrap()
            .ok
    );
    assert_eq!(files.disk(), "visible\nhidden\ntail\n");
    assert!(
        edit.invoke(json!({"path":"shared.txt", "old":"visible", "new":"edited"}))
            .await
            .unwrap()
            .ok
    );
    assert_eq!(files.disk(), "edited\nhidden\ntail\n");
}

#[tokio::test]
async fn paginated_reads_allow_full_replacement_only_after_complete_coverage() {
    let original = (0..2002)
        .map(|index| format!("line {index}\n"))
        .collect::<String>();
    let files = Files::new(&original);
    files.read().await; // actual default is 2000, not the full cached body.
    let write = WriteFileTool::with_cache(files.root.clone(), files.cache.clone());
    assert!(
        !write
            .invoke(json!({"path":"shared.txt", "content":"new"}))
            .await
            .unwrap()
            .ok
    );
    assert_eq!(files.disk(), original);
    assert!(
        files
            .reader()
            .invoke(json!({"path":"shared.txt", "offset":2001}))
            .await
            .unwrap()
            .ok
    );
    assert!(
        write
            .invoke(json!({"path":"shared.txt", "content":"new"}))
            .await
            .unwrap()
            .ok
    );
    assert_eq!(files.disk(), "new");
}

#[tokio::test]
async fn changed_version_does_not_inherit_observed_windows() {
    let files = Files::new("first\nsecond\n");
    files
        .reader()
        .invoke(json!({"path":"shared.txt", "limit":1}))
        .await
        .unwrap();
    std::fs::write(files.path(), "FIRST\nsecond\n").unwrap();
    files
        .reader()
        .invoke(json!({"path":"shared.txt", "offset":2}))
        .await
        .unwrap();
    let write = WriteFileTool::with_cache(files.root.clone(), files.cache.clone());
    assert!(
        !write
            .invoke(json!({"path":"shared.txt", "content":"new"}))
            .await
            .unwrap()
            .ok
    );
    assert_eq!(files.disk(), "FIRST\nsecond\n");
}

#[tokio::test]
async fn multi_edit_preserves_unseen_ranges_and_tracks_unicode_crlf_offsets() {
    let files = Files::new("可见一\r\n隐藏二\r\n可见三\r\n");
    for offset in [1, 3] {
        files
            .reader()
            .invoke(json!({"path":"shared.txt", "offset":offset,"limit":1}))
            .await
            .unwrap();
    }
    let edit = MultiEditTool::with_cache(files.root.clone(), files.cache.clone());
    let rejected = edit
        .invoke(json!({"path":"shared.txt", "edits":[
            {"old":"可见一","new":"更长的一\n新增行"}, {"old":"隐藏二", "new":"不应修改"}
        ]}))
        .await
        .unwrap();
    assert!(!rejected.ok);
    assert_eq!(files.disk(), "可见一\r\n隐藏二\r\n可见三\r\n");
    let accepted = edit.invoke(json!({"path":"shared.txt", "edits":[
        {"old":"可见一","new":"更长的一\n新增行"}, {"old":"新增行", "new":"新行"}, {"old":"可见三", "new":"末行"}
    ]})).await.unwrap();
    assert!(accepted.ok, "{accepted:?}");
    assert_eq!(files.disk(), "更长的一\r\n新行\r\n隐藏二\r\n末行\r\n");
}

#[tokio::test]
async fn replace_all_must_not_modify_unseen_occurrences() {
    let files = Files::new("match\nmatch\n");
    files
        .reader()
        .invoke(json!({"path":"shared.txt", "limit":1}))
        .await
        .unwrap();
    let edit = EditFileTool::with_cache(files.root.clone(), files.cache.clone());
    let result = edit
        .invoke(json!({"path":"shared.txt","old":"match","new":"changed","replace_all":true}))
        .await
        .unwrap();
    assert!(!result.ok);
    assert_eq!(files.disk(), "match\nmatch\n");
}

#[tokio::test]
async fn empty_observed_file_can_be_written_but_invalid_utf8_cannot() {
    let files = Files::new("");
    files.read().await;
    assert!(files.change(0, INITIAL).await.ok);
    std::fs::write(files.path(), [0xff, 0xfe]).unwrap();
    assert!(
        !files
            .reader()
            .invoke(json!({"path":"shared.txt"}))
            .await
            .unwrap()
            .ok
    );
    assert!(!files.change(0, INITIAL).await.ok);
    assert_eq!(std::fs::read(files.path()).unwrap(), [0xff, 0xfe]);
}

#[test]
fn a_change_during_staging_is_detected_before_publication() {
    let files = Files::new(INITIAL);
    let target = files.root.write_target(&files.path(), false).unwrap();
    let result = publish_checked(
        &target,
        Some(INITIAL),
        b"new",
        || std::fs::write(files.path(), EXTERNAL),
        StagedWrite::publish,
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("FILE_CHANGED_SINCE_READ"));
    assert_eq!(files.disk(), EXTERNAL);
    no_staging(files.root.path());
}

#[test]
fn a_new_target_created_after_final_check_is_never_clobbered() {
    let files = Files::new(INITIAL);
    let destination = files.root.path().join("new.txt");
    let target = files.root.write_target(&destination, false).unwrap();
    let result = publish_checked(
        &target,
        None,
        b"new",
        || Ok(()),
        |source, target, replace| {
            assert!(!replace);
            std::fs::write(&destination, EXTERNAL)?;
            source.publish(target, replace)
        },
    );
    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(destination).unwrap(), EXTERNAL);
    no_staging(files.root.path());
}

#[test]
fn preparation_publication_and_unwind_failures_preserve_disk_and_remove_staging() {
    let files = Files::new(INITIAL);
    let target = files.root.write_target(&files.path(), false).unwrap();
    assert!(publish_checked(
        &target,
        Some(INITIAL),
        b"new",
        || Err(io::Error::other("fixture preparation failure")),
        StagedWrite::publish
    )
    .is_err());
    assert!(publish_checked(
        &target,
        Some(INITIAL),
        b"new",
        || Ok(()),
        |_, _, _| Err(io::Error::other("fixture publish failure"))
    )
    .is_err());
    let panic = std::panic::catch_unwind(|| {
        let _ = publish_checked(
            &target,
            Some(INITIAL),
            b"new",
            || panic!("fixture after staging"),
            StagedWrite::publish,
        );
    });
    assert!(panic.is_err());
    assert_eq!(files.disk(), INITIAL);
    no_staging(files.root.path());
}

#[tokio::test]
async fn independent_caches_cannot_both_publish_the_same_observed_version() {
    for _ in 0..12 {
        let files = Files::new(INITIAL);
        files.read().await;
        let second_cache = Arc::new(Mutex::new(FileStateCache::new()));
        let reader = ReadFileTool::with_cache(files.root.clone(), second_cache.clone());
        assert!(
            reader
                .invoke(json!({"path":"shared.txt"}))
                .await
                .unwrap()
                .ok
        );
        let first = WriteFileTool::with_cache(files.root.clone(), files.cache.clone());
        let second = WriteFileTool::with_cache(files.root.clone(), second_cache);
        let (first, second) = tokio::join!(
            first.invoke(json!({"path":"shared.txt", "content":"first writer"})),
            second.invoke(json!({"path":"shared.txt", "content":"second writer"})),
        );
        let (first, second) = (first.unwrap(), second.unwrap());
        assert_ne!(first.ok, second.ok);
        assert_eq!(
            files.disk(),
            if first.ok {
                "first writer"
            } else {
                "second writer"
            }
        );
    }
}

struct ChildOwner(std::process::Child);
impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn file_mutation_child() {
    let Some(root) = std::env::var_os("DEEPAGENT_MUTATION_TEST_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let lock_only = std::env::var_os("DEEPAGENT_MUTATION_TEST_LOCK_ONLY")
        .map(|_| lock_path_operation(&root.join("shared.txt")).unwrap());
    if lock_only.is_some() {
        std::fs::write(root.join("child-ready"), b"ready").unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !root.join("child-go").exists() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        return;
    }
    let cache = Arc::new(Mutex::new(FileStateCache::new()));
    let read = ReadFileTool::with_cache(WorkspaceRoot::new(&root), cache.clone());
    assert!(read.invoke(json!({"path":"shared.txt"})).await.unwrap().ok);
    std::fs::write(root.join("child-ready"), b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !root.join("child-go").exists() {
        assert!(
            Instant::now() < deadline,
            "parent did not release child fixture"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let result = WriteFileTool::with_cache(WorkspaceRoot::new(&root), cache)
        .invoke(json!({"path":"shared.txt", "content":"child writer"}))
        .await
        .unwrap();
    if !result.ok {
        assert!(result.value["error"]
            .as_str()
            .unwrap()
            .contains("FILE_CHANGED_SINCE_READ"));
    }
    std::fs::write(
        root.join("child-outcome"),
        if result.ok { "ok" } else { "conflict" },
    )
    .unwrap();
}

#[tokio::test]
async fn independent_host_processes_cannot_both_commit_the_same_version() {
    let files = Files::new(INITIAL);
    files.read().await;
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "file_mutation::tests::file_mutation_child",
            "--nocapture",
        ])
        .env("DEEPAGENT_MUTATION_TEST_ROOT", files.root.path())
        .env_remove("DEEPAGENT_MUTATION_TEST_LOCK_ONLY")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut child = ChildOwner(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !files.root.path().join("child-ready").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before observation barrier"
        );
        assert!(Instant::now() < deadline, "child did not read fixture");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    std::fs::write(files.root.path().join("child-go"), b"go").unwrap();
    let result = WriteFileTool::with_cache(files.root.clone(), files.cache.clone())
        .invoke(json!({"path":"shared.txt", "content":"parent writer"}))
        .await
        .unwrap();
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "child did not finish mutation");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(status.success());
    let child_ok =
        std::fs::read_to_string(files.root.path().join("child-outcome")).unwrap() == "ok";
    assert_ne!(result.ok, child_ok);
    assert_eq!(
        files.disk(),
        if result.ok {
            "parent writer"
        } else {
            "child writer"
        }
    );
}

#[test]
fn actual_process_owner_excludes_competitors_and_releases_on_forced_exit() {
    let files = Files::new(INITIAL);
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "file_mutation::tests::file_mutation_child",
            "--nocapture",
        ])
        .env("DEEPAGENT_MUTATION_TEST_ROOT", files.root.path())
        .env("DEEPAGENT_MUTATION_TEST_LOCK_ONLY", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut child = ChildOwner(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !files.root.path().join("child-ready").exists() {
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let conflict = match lock_path_operation(&files.path()) {
        Ok(_) => panic!("live child owner was not excluded"),
        Err(error) => error,
    };
    assert_eq!(conflict.kind(), io::ErrorKind::WouldBlock);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    drop(lock_path_operation(&files.path()).unwrap());
    assert_eq!(files.disk(), INITIAL);
}

#[tokio::test]
async fn cancellation_before_entry_and_while_waiting_does_not_publish() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let files = Files::new(INITIAL);
    files.read().await;
    let flag = Arc::new(AtomicBool::new(true));
    let context = ToolExecutionContext::new(flag.clone());
    let write = WriteFileTool::with_cache(files.root.clone(), files.cache.clone());
    let result = write
        .invoke_with_context(
            json!({"path":"shared.txt", "content":"no"}),
            context.clone(),
        )
        .await
        .unwrap();
    assert!(!result.ok);
    assert_eq!(files.disk(), INITIAL);
    flag.store(false, Ordering::Release);
    let operation = lock_path_operation(&files.path()).unwrap();
    let pending = tokio::spawn(async move {
        write
            .invoke_with_context(json!({"path":"shared.txt", "content":"also no"}), context)
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!pending.is_finished());
    flag.store(true, Ordering::Release);
    drop(operation);
    let result = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert!(!result.ok);
    assert!(result.value["error"]
        .as_str()
        .unwrap()
        .contains("FILE_OPERATION_CANCELLED"));
    assert_eq!(files.disk(), INITIAL);
    no_staging(files.root.path());
    assert!(files.change(0, INITIAL).await.ok);
}
