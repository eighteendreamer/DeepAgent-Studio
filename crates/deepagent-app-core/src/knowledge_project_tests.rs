//! Real temporary vaults; no alternate memory store or user data roots.
use super::*;
use deepagent_runtime::{workflow::KnowledgeRetriever, RelevantMemoryProvider};

fn note(title: &str, body: &str, scope: &str) -> KnowledgeDraftDto {
    KnowledgeDraftDto {
        title: title.into(),
        body: body.into(),
        scope: Some(scope.into()),
        kind: Some("memory".into()),
        tags: vec![],
        source_session: None,
        tier: Some("workspace".into()),
        importance: Some(0.8),
    }
}

#[test]
fn project_views_share_one_cache_across_aliases_and_directory_creation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    let service = KnowledgeService::open(&root, &tmp.path().join("global")).unwrap();
    assert!(!root.exists(), "binding must not manufacture a project");
    let first = service.bind_current();
    #[cfg(windows)]
    let missing_alias = service
        .for_project(&PathBuf::from(root.to_string_lossy().to_uppercase()))
        .unwrap();
    let entry = first
        .save(note("Project shared", "first version", "project"))
        .unwrap();
    #[cfg(windows)]
    assert_eq!(
        missing_alias.get(&entry.id),
        Some(entry.clone()),
        "a handle bound before directory creation must observe its project's persisted write"
    );
    let alias = service.for_project(&root.join(".")).unwrap();
    assert_eq!(first.project, alias.project);
    assert_eq!(alias.get(&entry.id), Some(entry));
    let updated = alias
        .save(note("Project shared", "second version", "project"))
        .unwrap();
    assert_eq!(first.get(&updated.id), Some(updated));
    #[cfg(windows)]
    assert_eq!(
        first.project,
        service
            .for_project(&PathBuf::from(root.to_string_lossy().to_uppercase()))
            .unwrap()
            .project
    );
    assert_eq!(
        service.lock().bases.len(),
        1,
        "aliases must not fork a stale cache"
    );
    let b = tmp.path().join("b");
    service.activate_project(&b).unwrap();
    assert!(service.get("project:project-shared").is_none());
    first
        .save(note("Still A", "late project write", "project"))
        .unwrap();
    assert_eq!(first.entry_count(), 2);
    assert_eq!(service.entry_count(), 0);
    assert!(first.activate_project(&b).is_err());
    service.activate_project(&root).unwrap();
    assert_eq!(service.list(), first.list());
}

#[test]
fn materializing_projects_follow_real_filesystem_identity_for_either_first_writer() {
    for uppercase_first in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let lower = tmp.path().join("project");
        let upper = tmp.path().join("PROJECT");
        let global = tmp.path().join("global");
        let service = KnowledgeService::open(&lower, &global).unwrap();
        let left = service.bind_current();
        let right = service.for_project(&upper).unwrap();
        assert!(!lower.exists() && !upper.exists());
        let (first, second) = if uppercase_first {
            (&right, &left)
        } else {
            (&left, &right)
        };
        let one = first
            .save(note("First writer", "first evidence", "project"))
            .unwrap();
        let two = second
            .save(note("Second writer", "second evidence", "project"))
            .unwrap();
        // The actual filesystem decides whether these names identify one or
        // two directories. No OS-wide case-folding assumption enters the code.
        let same = std::fs::canonicalize(&lower).unwrap() == std::fs::canonicalize(&upper).unwrap();
        assert_eq!(first.get(&one.id), Some(one.clone()));
        assert_eq!(second.get(&two.id), Some(two.clone()));
        if same {
            assert_eq!(first.get(&two.id), Some(two));
            assert_eq!(second.get(&one.id), Some(one));
            assert_eq!(first.list(), second.list());
        } else {
            assert!(first.get(&two.id).is_none());
            assert!(second.get(&one.id).is_none());
        }
        assert_eq!(service.lock().bases.len(), if same { 1 } else { 2 });
        for (root, view) in [(&lower, &left), (&upper, &right)] {
            assert_eq!(service.for_project(root).unwrap().list(), view.list());
            assert_eq!(
                KnowledgeService::open(root, &global).unwrap().list(),
                view.list()
            );
            service.activate_project(root).unwrap();
            assert_eq!(service.list(), view.list());
        }
        let state = service.lock();
        assert!(state.bases.values().all(|project| project.materialized));
        assert!(state.aliases.iter().all(|(alias, target)| {
            !state.bases.contains_key(alias) && state.bases.contains_key(target)
        }));
    }
}

#[test]
fn materialization_reconciles_committed_project_write_even_when_operation_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let lower = tmp.path().join("project");
    let upper = tmp.path().join("PROJECT");
    let service = KnowledgeService::open(&lower, &tmp.path().join("global")).unwrap();
    let first = service.bind_current();
    let other = service.for_project(&upper).unwrap();
    let result: Result<()> = other.mutate(|base| {
        base.write(
            KnowledgeDraft {
                title: "Committed project".into(),
                body: "persisted before failure".into(),
                scope: Scope::Project,
                kind: EntryKind::Memory,
                tags: vec![],
                source_session: None,
                tier: Some("workspace".into()),
                importance: None,
            },
            1,
        )?;
        Err(CoreError::other("failure after project write"))
    });
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("failure after project write"));
    let committed = other.get("project:committed-project").unwrap();
    match std::fs::canonicalize(&lower) {
        Ok(physical) => {
            assert_eq!(physical, std::fs::canonicalize(&upper).unwrap());
            assert_eq!(first.get(&committed.id), Some(committed));
            assert_eq!(service.lock().bases.len(), 1);
        }
        Err(error) => {
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
            assert!(first.get(&committed.id).is_none());
            assert_eq!(service.lock().bases.len(), 2);
        }
    }
}

#[test]
fn concurrent_pending_case_alias_writers_preserve_colliding_titles_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let lower = tmp.path().join("project");
    let upper = tmp.path().join("PROJECT");
    let global = tmp.path().join("global");
    let service = KnowledgeService::open(&lower, &global).unwrap();
    let left = service.bind_current();
    let right = service.for_project(&upper).unwrap();
    let barrier = std::sync::Barrier::new(3);
    std::thread::scope(|scope| {
        for (view, punctuation) in [(&left, ":"), (&right, "")] {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for index in 0..8 {
                    let title = format!("Policy{punctuation} {index}");
                    view.save(note(&title, &title, "project")).unwrap();
                }
            });
        }
        barrier.wait();
    });
    let same = std::fs::canonicalize(&lower).unwrap() == std::fs::canonicalize(&upper).unwrap();
    for (root, view) in [(&lower, &left), (&upper, &right)] {
        assert_eq!(view.entry_count(), if same { 16 } else { 8 });
        assert_eq!(
            KnowledgeService::open(root, &global).unwrap().list(),
            view.list()
        );
    }
    assert_eq!(service.lock().bases.len(), if same { 1 } else { 2 });
}

#[cfg(windows)]
#[test]
fn materialized_pending_project_freezes_its_physical_vault_target() {
    struct Junction(PathBuf);
    impl Drop for Junction {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    let junction = |path: &Path, target: &Path| {
        // Same temporary-directory fixture as kernel_v2_e2e/path_boundary.rs.
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(path)
            .arg(target)
            .output()
            .unwrap();
        assert!(output.status.success(), "mklink /J failed: {output:?}");
    };
    let tmp = tempfile::tempdir().unwrap();
    let logical = tmp.path().join("pending");
    let alpha = tmp.path().join("alpha");
    let beta = tmp.path().join("beta");
    let global = tmp.path().join("global");
    let service = KnowledgeService::open(&logical, &global).unwrap();
    let bound = service.bind_current();
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::create_dir_all(&beta).unwrap();
    junction(&logical, &alpha);
    let _guard = Junction(logical.clone());
    bound
        .save(note("Original", "original alpha", "project"))
        .unwrap();
    // After identity materializes, replacing the original logical link must
    // not retarget the already-bound cache's actual vault I/O to beta.
    std::fs::remove_dir(&logical).unwrap();
    junction(&logical, &beta);
    let late = bound.save(note("Late", "late alpha", "project")).unwrap();
    assert_eq!(
        KnowledgeService::open(&alpha, &global)
            .unwrap()
            .get(&late.id),
        Some(late)
    );
    assert!(KnowledgeService::open(&beta, &global)
        .unwrap()
        .list()
        .is_empty());
    // Reusing the old logical name for a new real directory is a new project,
    // not permission for a new explicit binding to inherit alpha's old cache.
    std::fs::remove_dir(&logical).unwrap();
    std::fs::create_dir_all(&logical).unwrap();
    let rebound = service.for_project(&logical).unwrap();
    assert!(
        rebound.list().is_empty(),
        "new physical project inherited old logical cache"
    );
    let fresh = rebound
        .save(note("Fresh", "new logical project", "project"))
        .unwrap();
    assert!(bound.get(&fresh.id).is_none());
    assert!(KnowledgeService::open(&alpha, &global)
        .unwrap()
        .get(&fresh.id)
        .is_none());
    assert_eq!(
        KnowledgeService::open(&logical, &global)
            .unwrap()
            .get(&fresh.id),
        Some(fresh)
    );
}

#[test]
fn unrelated_invalid_pending_project_does_not_block_a_healthy_bound_project() {
    let tmp = tempfile::tempdir().unwrap();
    let healthy = tmp.path().join("healthy");
    std::fs::create_dir_all(&healthy).unwrap();
    let service = KnowledgeService::open(&healthy, &tmp.path().join("global")).unwrap();
    let bound = service.bind_current();
    let invalid = tmp.path().join("pending");
    let other = service.for_project(&invalid).unwrap();
    std::fs::write(&invalid, "not a directory").unwrap();
    let entry = bound
        .save(note("Healthy", "healthy private write", "project"))
        .unwrap();
    assert_eq!(service.get(&entry.id), Some(entry));
    assert!(other.save(note("Invalid", "must fail", "project")).is_err());
    assert!(service.activate_project(&invalid).is_err());
    assert_eq!(service.list(), bound.list());
    assert!(!healthy.join(".deepagent/knowledge/invalid.md").exists());
}

#[test]
fn global_mutations_publish_entries_drafts_deletions_and_reload_to_all_views() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    let global = tmp.path().join("global");
    let service = KnowledgeService::open(&a, &global).unwrap();
    let left = service.bind_current();
    let right = service.for_project(&b).unwrap();
    let private = right
        .save(note("Private B", "private beta", "project"))
        .unwrap();
    let first = left
        .save(note("Global: policy", "shared deployment policy", "global"))
        .unwrap();
    assert_eq!(right.get(&first.id), Some(first.clone()));
    assert!(right
        .search("shared deployment policy", None, 10)
        .iter()
        .any(|hit| hit.id == first.id));
    assert!(right
        .passive_block("shared deployment policy")
        .contains("shared deployment policy"));
    let second = right
        .save(note("Global policy", "a distinct policy", "global"))
        .unwrap();
    assert_ne!(
        first.id, second.id,
        "same-slug titles must not overwrite across project caches"
    );
    let updated = right
        .save(note(
            "Global: policy",
            "updated deployment policy",
            "global",
        ))
        .unwrap();
    assert_eq!(left.get(&first.id), Some(updated));
    assert!(left.get(&private.id).is_none());
    assert_eq!(
        KnowledgeService::open(&a, &global).unwrap().list(),
        left.list()
    );
    assert!(left.delete(&second.id).unwrap());
    assert!(right.get(&second.id).is_none());

    let draft = left
        .add_draft(KnowledgeDraft {
            title: "Pending shared".into(),
            body: "global pending evidence".into(),
            kind: EntryKind::Memory,
            scope: Scope::Global,
            tags: vec![],
            source_session: None,
            tier: None,
            importance: None,
        })
        .unwrap();
    assert_eq!(left.list_drafts(), right.list_drafts());
    assert!(!right
        .search("global pending evidence", None, 10)
        .iter()
        .any(|hit| hit.id == draft.id));
    let accepted = right.accept_draft(&draft.id).unwrap();
    assert_eq!(left.get(&draft.id), Some(accepted));
    assert!(left.list_drafts().is_empty());
    let discarded = right
        .add_draft(KnowledgeDraft {
            title: "Discard shared".into(),
            body: "not searchable".into(),
            kind: EntryKind::Note,
            scope: Scope::Global,
            tags: vec![],
            source_session: None,
            tier: None,
            importance: None,
        })
        .unwrap();
    assert!(left.discard_draft(&discarded.id).unwrap());
    assert!(right.list_drafts().is_empty());

    // An explicit reload is the only external-writer visibility promise here.
    let external = KnowledgeService::open(&b, &global).unwrap();
    let imported = external
        .save(note("Reloaded", "external update", "global"))
        .unwrap();
    left.reload().unwrap();
    assert_eq!(right.get(&imported.id), Some(imported));
    assert_eq!(right.get(&private.id), Some(private));
}

#[test]
fn global_consolidation_and_partial_error_publish_without_copying_private_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let service =
        KnowledgeService::open(&tmp.path().join("a"), &tmp.path().join("global")).unwrap();
    let left = service.bind_current();
    let right = service.for_project(&tmp.path().join("b")).unwrap();
    let private = right
        .save(note("Private B", "beta only", "project"))
        .unwrap();
    let body = "shared deployment evidence repeated verbatim for consolidation";
    left.save(note("Deployment one", body, "global")).unwrap();
    right.save(note("Deployment two", body, "global")).unwrap();
    let report = left
        .consolidate(None, SystemClock.now().as_millis())
        .unwrap();
    assert_eq!(report.superseded.len(), 1);
    assert_eq!(
        left.list(),
        right
            .list()
            .into_iter()
            .filter(|entry| entry.scope == "global")
            .collect::<Vec<_>>()
    );
    // Exercise the host mutation boundary when an operation reports failure
    // after its first real persisted write; committed data must still publish.
    let error: Result<()> = left.mutate(|base| {
        base.write(
            KnowledgeDraft {
                title: "Committed before error".into(),
                body: "persisted data".into(),
                kind: EntryKind::Note,
                scope: Scope::Global,
                tags: vec![],
                source_session: None,
                tier: None,
                importance: None,
            },
            100,
        )?;
        Err(CoreError::other(
            "test operation failed after the committed write",
        ))
    });
    assert!(error.is_err());
    assert_eq!(
        left.get("global:committed-before-error"),
        right.get("global:committed-before-error")
    );
    assert!(right.get("global:committed-before-error").is_some());
    assert_eq!(right.get(&private.id), Some(private));
}

#[test]
fn concurrent_project_and_global_writers_keep_every_distinct_note_on_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    let global = tmp.path().join("global");
    let service = KnowledgeService::open(&a, &global).unwrap();
    let left = service.bind_current();
    let right = service.for_project(&b).unwrap();
    let barrier = std::sync::Barrier::new(3);
    std::thread::scope(|scope| {
        for (view, punctuation) in [(&left, ":"), (&right, "")] {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for index in 0..12 {
                    let title = format!("Shared{punctuation} number {index}");
                    view.save(note(&title, &title, "global")).unwrap();
                    view.save(note(&format!("Private {index}"), punctuation, "project"))
                        .unwrap();
                }
            });
        }
        barrier.wait();
    });
    for (path, view, body) in [(&a, &left, ":"), (&b, &right, "")] {
        let reopened = KnowledgeService::open(path, &global).unwrap();
        assert_eq!(reopened.list(), view.list());
        assert_eq!(view.entry_count(), 36);
        assert!(view
            .list()
            .iter()
            .filter(|entry| entry.scope == "project")
            .all(|entry| entry.body == body));
    }
}

#[tokio::test]
async fn delayed_background_and_workflow_adapters_keep_the_construction_project() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    let service = Arc::new(KnowledgeService::open(&a, &tmp.path().join("global")).unwrap());
    let query = "deployment workspace routing evidence";
    service
        .save(note("Alpha", &format!("{query} OWNER_ALPHA"), "project"))
        .unwrap();
    let provider = KnowledgeMemoryProvider::new(service.clone());
    let workflow = WorkflowKnowledgeRetriever::new(service.clone());
    let backend = KnowledgeServiceBackend::new(service.clone());
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let pending = tokio::spawn(async move {
        gate.notified().await;
        provider.fetch_relevant(query, &[], &[]).await.unwrap()
    });
    service.activate_project(&b).unwrap();
    service
        .save(note("Beta", &format!("{query} OWNER_BETA"), "project"))
        .unwrap();
    release.notify_one();
    let fetched = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert!(fetched.iter().any(|hit| hit.block.contains("OWNER_ALPHA")));
    assert!(!fetched.iter().any(|hit| hit.block.contains("OWNER_BETA")));
    let documents = workflow.search(query, 10).await.unwrap();
    assert!(documents
        .iter()
        .any(|hit| hit.content.contains("OWNER_ALPHA")));
    assert!(!documents
        .iter()
        .any(|hit| hit.content.contains("OWNER_BETA")));
    use deepagent_builtins::KnowledgeBackend;
    let hits = backend.search(query, None, 10).await.unwrap();
    assert!(hits.iter().any(|hit| hit.excerpt.contains("OWNER_ALPHA")));
    assert!(!hits.iter().any(|hit| hit.excerpt.contains("OWNER_BETA")));
}

#[test]
fn failed_activation_preserves_selection_and_runtime_toggles_remain_shared() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let service = KnowledgeService::open(&a, &tmp.path().join("global")).unwrap();
    let left = service.bind_current();
    let right = service.for_project(&tmp.path().join("b")).unwrap();
    let blocker = tmp.path().join("not-a-directory");
    std::fs::write(&blocker, "not a vault").unwrap();
    assert!(service.activate_project(&blocker.join("child")).is_err());
    assert_eq!(service.bind_current().project, left.project);
    service.set_passive_enabled(false);
    right.set_auto_capture(false);
    for view in [
        &left,
        &right,
        &service.for_project(&tmp.path().join("c")).unwrap(),
    ] {
        assert!(!view.passive_enabled());
        assert!(!view.auto_capture_enabled());
    }
    service.set_passive_enabled(true);
    assert!(left.passive_enabled());
    assert!(right.passive_enabled());
}

#[test]
fn ui_switch_rescans_existing_view_without_retargeting_a_live_handle() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    let global = tmp.path().join("global");
    let service = KnowledgeService::open(&a, &global).unwrap();
    let alpha = service.bind_current();
    service.activate_project(&b).unwrap();
    let beta = service.bind_current();
    let external = KnowledgeService::open(&a, &global).unwrap();
    let local = external
        .save(note(
            "Rescanned project",
            "external private update",
            "project",
        ))
        .unwrap();
    let shared = external
        .save(note("Rescanned global", "external shared update", "global"))
        .unwrap();
    service.activate_project(&a).unwrap();
    assert_eq!(alpha.get(&local.id), Some(local.clone()));
    assert_eq!(service.get(&local.id), Some(local));
    assert!(beta.get("project:rescanned-project").is_none());
    assert_eq!(beta.get(&shared.id), Some(shared));
}

#[test]
fn custom_base_remains_fixed_without_guessing_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let base = KnowledgeBase::new(
        vec![Vault::new(tmp.path().join("custom"), Scope::Project)],
        HashingEmbedder::default(),
        KnowledgeConfig::default(),
    );
    let service = KnowledgeService::from_base(base);
    let view = service.for_project(&tmp.path().join("unrelated")).unwrap();
    let entry = view.save(note("Custom", "custom root", "project")).unwrap();
    assert_eq!(service.get(&entry.id), Some(entry));
    assert!(!tmp.path().join("unrelated").exists());
    assert!(service.activate_project(tmp.path()).is_err());
}
