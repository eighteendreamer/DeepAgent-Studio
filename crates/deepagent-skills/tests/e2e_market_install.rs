//! End-to-end integration test for the marketplace install flow.
//!
//! Simulates the full Tauri command pipeline (`skill_market_scan` →
//! `skill_market_install` → `list_skills` → `skill_market_cancel`) using only
//! the kernel-level APIs in `deepagent-skills`. The networked half (codeload
//! / SkillsMP REST) is replaced by an in-memory zip fixture and a direct
//! call to [`marketplace::extract_skill_subtree`], so the test is fully
//! offline and deterministic.
//!
//! The flow exercised here mirrors what `apps/desktop/src-tauri/src/lib.rs`
//! does at runtime:
//!
//! 1. **Scan.** A codeload-shaped zip is built in memory (single top-level
//!    `{repo}-{branch}/` wrapper, with the skill nested at
//!    `{repo}-{branch}/skills/sample-skill/`). `extract_skill_subtree`
//!    unpacks it into a [`TempSkillDir`]; the caller then runs the static
//!    [`scan_dir`] over that tempdir to produce a [`ScanReport`]. The temp
//!    handle is parked in a `skills_pending`-style map keyed by an opaque
//!    `temp_id`, exactly the way the Tauri command layer parks them in
//!    `AppState.skills_pending`.
//!
//! 2. **Install.** `skill_market_install(temp_id)` pops the handle out of
//!    the map and invokes [`SkillManager::install`] with the temp's `root`,
//!    landing the skill under the simulated user home at
//!    `<home>/.deepagent/skills/marketplace/<id>/` and registering it with
//!    `origin = installed`. Dropping the popped temp handle cleans the
//!    unpacked tempdir.
//!
//! 3. **List.** `list_skills` projects the live registry's catalog into the
//!    same `(id, origin)` view the desktop UI reads from. The new skill must
//!    appear with `origin = "installed"`.
//!
//! 4. **Cancel after install.** Calling `skill_market_cancel(temp_id)` after
//!    the install completes must be a no-op: the temp handle is already
//!    gone, the underlying tempdir already dropped, and no error surfaces to
//!    the caller.
//!
//! _Validates: Requirements R3.6, R4.1, R4.10, R4.11._

use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use deepagent_skills::marketplace::{extract_skill_subtree, TempSkillDir};
use deepagent_skills::{scan_dir, ScanReport, SkillManager, SkillOrigin};

// ---------------------------------------------------------------------------
// Minimal SKILL.md fixture.
// ---------------------------------------------------------------------------

/// A tiny but valid SKILL.md frontmatter + body. Frontmatter omits
/// `disable-model-invocation`, so the skill defaults to model-invocable.
const FIXTURE_SKILL_MD: &str = "---\n\
name: Sample Skill\n\
description: A safe sample skill used by the marketplace e2e test. Use to \"summarize text\".\n\
version: 0.1.0\n\
---\n\
# Sample Skill\n\n\
A harmless skill body. No shell, no network, no credential reads.\n";

/// A plain Markdown reference (no risk patterns), placed under
/// `references/` so the fixture also exercises the loader's resource scanner.
const FIXTURE_USAGE_MD: &str = "# Usage\n\n\
Read the SKILL.md and follow the instructions.\n";

// ---------------------------------------------------------------------------
// Codeload-shaped zip builder.
// ---------------------------------------------------------------------------

/// Build an in-memory zip whose layout mirrors a real GitHub `codeload`
/// download:
///
/// ```text
/// {top_dir}/
///   {path_within}/
///     SKILL.md
///     references/usage.md
/// ```
///
/// `extract_skill_subtree` keys off the single top-level directory and the
/// `path_within` argument to identify the skill root, so we keep the wrapper
/// dir plus the path-within prefix on every entry.
fn build_codeload_zip(top_dir: &str, path_within: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::<u8>::new()));
    let opts: zip::write::FileOptions =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for (rel, bytes) in files {
        let entry_name = if path_within.is_empty() {
            format!("{}/{}", top_dir, rel)
        } else {
            format!("{}/{}/{}", top_dir, path_within, rel)
        };
        zip.start_file(entry_name, opts)
            .expect("start_file in fixture zip");
        zip.write_all(bytes).expect("write fixture entry bytes");
    }
    let cursor = zip.finish().expect("finish fixture zip");
    cursor.into_inner()
}

// ---------------------------------------------------------------------------
// Tauri-layer simulation: skills_pending + the four entry points.
// ---------------------------------------------------------------------------

/// Mirrors `AppState.skills_pending` in the Tauri layer (task 8). Each entry
/// owns a [`TempSkillDir`] whose internal `tempfile::TempDir` is dropped (and
/// the on-disk tempdir cleaned) when the entry is removed.
struct SkillsPending {
    inner: Mutex<HashMap<String, TempSkillDir>>,
}

impl SkillsPending {
    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn insert(&self, temp_id: String, dir: TempSkillDir) {
        self.inner
            .lock()
            .expect("skills_pending mutex poisoned")
            .insert(temp_id, dir);
    }

    fn take(&self, temp_id: &str) -> Option<TempSkillDir> {
        self.inner
            .lock()
            .expect("skills_pending mutex poisoned")
            .remove(temp_id)
    }
}

/// `skill_market_scan(github_url)` simulation. The real command parses the
/// URL via [`SkillsMpClient::parse_github_url`], streams the codeload zip,
/// and then calls `extract_skill_subtree` + `scan_dir`. Here we feed the
/// already-built fixture zip straight into `extract_skill_subtree` to keep
/// the test offline.
///
/// Returns the produced [`ScanReport`] together with the opaque `temp_id`
/// the caller would later pass to install / cancel.
fn skill_market_scan(
    pending: &SkillsPending,
    counter: &mut u64,
    zip_bytes: &[u8],
    path_within: &str,
) -> (ScanReport, String) {
    let temp = extract_skill_subtree(zip_bytes, path_within).expect("extract skill subtree");
    let report = scan_dir(&temp.root).expect("scan unpacked skill");
    *counter += 1;
    let temp_id = format!("test-temp-{counter:08x}");
    pending.insert(temp_id.clone(), temp);
    (report, temp_id)
}

/// `skill_market_install(temp_id)` simulation. Looks up the parked temp
/// handle, invokes the local installer (which copies into the marketplace
/// dir + registers the skill with `origin = installed`), and drops the temp
/// handle so the unpacked tempdir is cleaned up.
///
/// Panics if `temp_id` is not in `skills_pending` — the real command returns
/// an `Err`, but for this test we treat that as a fixture bug.
fn skill_market_install(
    pending: &SkillsPending,
    manager: &mut SkillManager,
    temp_id: &str,
) -> deepagent_skills::SkillMeta {
    let temp = pending
        .take(temp_id)
        .unwrap_or_else(|| panic!("temp_id {temp_id} missing from skills_pending"));
    let meta = manager.install(&temp.root).expect("install from temp");
    drop(temp);
    meta
}

/// `skill_market_cancel(temp_id)` simulation. The contract from R4.9 / task 8
/// is "user取消,清临时目录" — we drop whatever handle is still parked under
/// `temp_id`. After install the entry is already gone, so this is a no-op
/// and must not error.
fn skill_market_cancel(pending: &SkillsPending, temp_id: &str) {
    // `HashMap::remove` returns `Option<_>`; missing entries surface as
    // `None`, never as an error.
    let _ = pending.take(temp_id);
}

/// `list_skills` simulation. The real command projects through
/// [`crate::skills_service::SkillsService::list`]; the underlying source of
/// truth is the live [`SkillRegistry`][deepagent_skills::SkillRegistry]
/// catalog. Project to `(id, origin_label)` so the test asserts the same
/// shape the desktop UI consumes.
fn list_skills(manager: &SkillManager) -> Vec<(String, String)> {
    manager
        .registry()
        .catalog()
        .into_iter()
        .map(|m| (m.id.clone(), m.origin.label().to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Test layout.
// ---------------------------------------------------------------------------

/// Build a `~/.deepagent/skills/marketplace/` layout under a fresh tempdir
/// playing the role of the user's home directory.
struct FakeHome {
    _root: tempfile::TempDir,
    marketplace: PathBuf,
}

impl FakeHome {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create fake home tempdir");
        let marketplace = root
            .path()
            .join(".deepagent")
            .join("skills")
            .join("marketplace");
        std::fs::create_dir_all(&marketplace).expect("create marketplace root");
        Self {
            _root: root,
            marketplace,
        }
    }
}

// ---------------------------------------------------------------------------
// E2E test.
// ---------------------------------------------------------------------------

/// _Validates: Requirements R3.6, R4.1, R4.10, R4.11._
#[test]
fn scan_install_list_then_cancel_is_noop() {
    let home = FakeHome::new();

    // The Tauri shell builds the manager over the marketplace dir; the
    // workspace dir is `None` for this test (no project-local skills).
    let mut manager = SkillManager::new(None, &home.marketplace);
    let pending = SkillsPending::new();
    let mut temp_id_counter = 0u64;

    // Sanity: starting state — no skills registered, marketplace dir empty.
    assert!(
        list_skills(&manager).is_empty(),
        "fresh manager registers nothing"
    );
    assert!(
        std::fs::read_dir(&home.marketplace)
            .expect("read marketplace dir")
            .next()
            .is_none(),
        "marketplace dir starts empty"
    );

    // ----- 1. skill_market_scan(github_url) -------------------------------
    //
    // Build a codeload-shaped zip carrying our sample skill at
    // `sample-repo-main/skills/sample-skill/...`. Path-within is
    // `skills/sample-skill`, matching what
    // `SkillsMpClient::parse_github_url` would extract from a `…/tree/main/
    // skills/sample-skill` GitHub URL.
    let zip_bytes = build_codeload_zip(
        "sample-repo-main",
        "skills/sample-skill",
        &[
            ("SKILL.md", FIXTURE_SKILL_MD.as_bytes()),
            ("references/usage.md", FIXTURE_USAGE_MD.as_bytes()),
        ],
    );

    let (report, temp_id) = skill_market_scan(
        &pending,
        &mut temp_id_counter,
        &zip_bytes,
        "skills/sample-skill",
    );

    // R4.1 / R4.2: the scan must produce a ScanReport before any install
    // happens. The fixture is intentionally safe — no risks.
    assert_eq!(report.name, "sample-skill");
    assert!(
        report.skill_md_content.contains("Sample Skill"),
        "scan report should embed full SKILL.md, got: {:?}",
        report.skill_md_content
    );
    assert_eq!(
        report.files.len(),
        2,
        "fixture has SKILL.md + references/usage.md, got {:?}",
        report.files
    );
    assert!(
        report.risks.is_empty(),
        "safe fixture should produce zero risks, got {:?}",
        report.risks
    );
    assert!(!temp_id.is_empty(), "scan must hand back an opaque temp_id");

    // ----- 2. skill_market_install(temp_id) -------------------------------
    let meta = skill_market_install(&pending, &mut manager, &temp_id);
    assert_eq!(meta.id, "sample-skill");
    assert_eq!(meta.origin, SkillOrigin::Installed);
    assert_eq!(meta.version.as_deref(), Some("0.1.0"));

    // R3.6 / R2.3: the skill must land under the marketplace root, not the
    // tempdir.
    let installed_root = home.marketplace.join("sample-skill");
    assert!(
        installed_root.is_dir(),
        "skill dir should exist at {}",
        installed_root.display()
    );
    assert!(
        installed_root.join("SKILL.md").is_file(),
        "SKILL.md missing from installed skill at {}",
        installed_root.display()
    );
    assert!(
        installed_root.join("references").join("usage.md").is_file(),
        "reference asset missing from installed skill at {}",
        installed_root.display()
    );

    // ----- 3. list_skills -------------------------------------------------
    let listed = list_skills(&manager);
    assert_eq!(
        listed.len(),
        1,
        "exactly one skill registered, got {listed:?}"
    );
    let (id, origin) = &listed[0];
    assert_eq!(id, "sample-skill");
    assert_eq!(
        origin, "installed",
        "origin must be `installed` for marketplace skills"
    );

    // And: a fresh manager pointed at the same marketplace dir reloads the
    // installed skill from disk — proves the install actually persisted to
    // `<home>/.deepagent/skills/marketplace/`, not just to the registry.
    let mut reloaded = SkillManager::new(None, &home.marketplace);
    let n = reloaded.load_all().expect("reload installed skills");
    assert_eq!(
        n, 1,
        "reloaded manager should see exactly the installed skill"
    );
    let reloaded_listing = list_skills(&reloaded);
    assert_eq!(
        reloaded_listing,
        vec![("sample-skill".to_string(), "installed".to_string())]
    );

    // ----- 4. skill_market_cancel(temp_id) AFTER install ------------------
    //
    // The contract: cancelling a temp_id that has already been consumed by
    // install must NOT error and must NOT panic. The simulation drops a
    // missing key from `skills_pending`, which mirrors what the real Tauri
    // command does.
    skill_market_cancel(&pending, &temp_id);

    // The marketplace state is unchanged after the cancel.
    let listed_after_cancel = list_skills(&manager);
    assert_eq!(
        listed_after_cancel,
        vec![("sample-skill".to_string(), "installed".to_string())],
        "cancel-after-install must not disturb the registry"
    );
    assert!(
        installed_root.join("SKILL.md").is_file(),
        "cancel-after-install must not touch the installed skill on disk"
    );
}
