//! Skill management for the UI.
//!
//! Wraps [`deepagent_skills::SkillManager`] and exposes serializable DTOs so the
//! desktop app can list, install, uninstall, and preview activation of skills.
//! The networked "download" step (fetch a skill archive from a URL/marketplace)
//! is performed by the app shell; this service consumes an already-unpacked
//! source directory via [`SkillsService::install_from_dir`].

use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};
use deepagent_skills::{
    loader, RiskCategory, RiskSeverity, ScanReport, SkillManager, SkillMeta, SkillOrigin,
    SkillsRoots,
};
use serde::{Deserialize, Serialize};

use crate::chat_service::ChatService;

/// A serializable view of a skill's Level-1 metadata for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDto {
    /// Stable skill id (slug).
    pub id: String,
    /// Human name.
    pub name: String,
    /// Description (with trigger phrases).
    pub description: String,
    /// Optional version.
    pub version: Option<String>,
    /// Origin label (workspace/user/installed/built_in).
    pub origin: String,
    /// Trigger phrases used for passive activation.
    pub triggers: Vec<String>,
}

impl SkillDto {
    fn from_meta(meta: &SkillMeta, triggers: Vec<String>) -> Self {
        Self {
            id: meta.id.clone(),
            name: meta.name.clone(),
            description: meta.description.clone(),
            version: meta.version.clone(),
            origin: meta.origin.label().to_string(),
            triggers,
        }
    }
}

/// The result of a passive-activation preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillActivationDto {
    /// The matched skill id.
    pub id: String,
    /// The disclosed Level-2 body (the prompt fragment content).
    pub body: String,
}

/// UI-facing skill management service.
pub struct SkillsService {
    manager: SkillManager,
    roots: Option<SkillsRoots>,
    plugin_roots: Vec<PathBuf>,
}

impl SkillsService {
    /// Build over an optional workspace skills dir and a managed install root,
    /// loading all discoverable + installed skills immediately.
    pub fn open(workspace_dir: Option<PathBuf>, install_dir: impl Into<PathBuf>) -> Result<Self> {
        let mut manager = SkillManager::new(workspace_dir, install_dir);
        manager.load_all()?;
        Ok(Self {
            manager,
            roots: None,
            plugin_roots: Vec::new(),
        })
    }

    /// Build over the four-tier marketplace storage layout
    /// ([`SkillsRoots`]). Each root is scanned recursively up to depth 3 so
    /// nested layouts like `superpowers/skills/<sub>/SKILL.md` are picked up.
    /// Skills from each root are registered in priority-ascending order
    /// (BuiltIn → Installed → User → Workspace) so that on id conflict the
    /// higher-priority origin wins.
    ///
    /// Missing roots (the marketplace dir doesn't exist yet, the workspace is
    /// `None`, etc.) are handled gracefully: they contribute zero skills, no
    /// error.
    pub fn open_v2(roots: SkillsRoots) -> Result<Self> {
        Ok(Self {
            manager: Self::build_v2_manager(&roots, &[])?,
            roots: Some(roots),
            plugin_roots: Vec::new(),
        })
    }

    fn build_v2_manager(roots: &SkillsRoots, plugin_roots: &[PathBuf]) -> Result<SkillManager> {
        // The legacy install/uninstall path needs an install_dir; the
        // marketplace path is the natural place to land newly-installed skills.
        let mut manager = SkillManager::new(None, &roots.marketplace);

        // If the marketplace directory is nested under the user root (the
        // documented layout: `~/.deepagent/skills/marketplace/`), make sure the
        // user-root scan doesn't double-pick the same skills with origin=User.
        let user_excludes: Vec<PathBuf> = if roots.marketplace.starts_with(&roots.user) {
            vec![roots.marketplace.clone()]
        } else {
            Vec::new()
        };

        const MAX_DEPTH: usize = 3;

        // Priority-ascending registration: each later origin overwrites an
        // earlier same-id entry via SkillRegistry::register's replace semantics.
        for skill in loader::discover_recursive(&roots.builtin, SkillOrigin::BuiltIn, MAX_DEPTH)? {
            manager.register(skill);
        }
        for skill in
            loader::discover_recursive(&roots.marketplace, SkillOrigin::Installed, MAX_DEPTH)?
        {
            manager.register(skill);
        }
        for skill in loader::discover_recursive_excluding(
            &roots.user,
            SkillOrigin::User,
            MAX_DEPTH,
            &user_excludes,
        )? {
            manager.register(skill);
        }
        if let Some(ws) = roots.workspace.as_ref() {
            for skill in loader::discover_recursive(ws, SkillOrigin::Workspace, MAX_DEPTH)? {
                manager.register(skill);
            }
        }
        for root in plugin_roots {
            for skill in loader::discover_recursive(root, SkillOrigin::Plugin, MAX_DEPTH)? {
                manager.register(skill);
            }
        }
        Ok(manager)
    }

    /// Build over an existing manager (e.g. for tests).
    pub fn from_manager(manager: SkillManager) -> Self {
        Self {
            manager,
            roots: None,
            plugin_roots: Vec::new(),
        }
    }

    /// Read-only access to the underlying [`SkillManager`].
    ///
    /// The chat-service ([`crate::chat_service::ChatService`]) reaches for
    /// this when wiring up the catalog reminder + `skill` tool: it needs to
    /// snapshot the live [`SkillRegistry`][deepagent_skills::SkillRegistry]
    /// once per run. The accessor returns a borrow rather than the registry
    /// directly so consumers go through the manager's stable surface.
    pub fn manager(&self) -> &SkillManager {
        &self.manager
    }

    pub fn plugin_roots(&self) -> &[PathBuf] {
        &self.plugin_roots
    }

    /// Reload all skills from disk.
    pub fn reload(&mut self) -> Result<usize> {
        if let Some(roots) = &self.roots {
            self.manager = Self::build_v2_manager(roots, &self.plugin_roots)?;
            Ok(self.manager.registry().len())
        } else {
            self.manager.load_all()
        }
    }

    /// Replace the runtime plugin skill roots and reload the registry when
    /// they changed. Plugin skills are registered after workspace skills, so an
    /// enabled plugin can intentionally override a lower-priority same-id skill.
    pub fn set_plugin_roots(&mut self, roots: Vec<PathBuf>) -> Result<usize> {
        if self.plugin_roots == roots {
            return Ok(self.manager.registry().len());
        }
        self.plugin_roots = roots;
        self.reload()
    }

    /// List all known skills as DTOs (sorted by id).
    pub fn list(&self) -> Vec<SkillDto> {
        self.manager
            .registry()
            .catalog()
            .into_iter()
            .map(|meta| {
                let triggers = self
                    .manager
                    .registry()
                    .get(&meta.id)
                    .map(|s| s.triggers.clone())
                    .unwrap_or_default();
                SkillDto::from_meta(meta, triggers)
            })
            .collect()
    }

    /// Install a skill from an already-downloaded/unpacked source directory.
    pub fn install_from_dir(&mut self, source: impl AsRef<std::path::Path>) -> Result<SkillDto> {
        let meta = self.manager.install(source)?;
        let triggers = self
            .manager
            .registry()
            .get(&meta.id)
            .map(|s| s.triggers.clone())
            .unwrap_or_default();
        Ok(SkillDto::from_meta(&meta, triggers))
    }

    /// Install a skill from a temporary directory (typically returned by
    /// [`SkillsMpClient::download_skill_to_temp`][deepagent_skills::SkillsMpClient::download_skill_to_temp]).
    ///
    /// Validates that `temp` is a directory containing a top-level `SKILL.md`,
    /// then delegates to [`SkillsService::install_from_dir`] (which copies the
    /// directory into the marketplace root and registers the skill with
    /// `origin = "installed"`).
    ///
    /// _Validates: Requirements R1.3, R2.3, R3.6._
    pub fn install_from_temp(&mut self, temp: &Path) -> Result<SkillDto> {
        if !temp.is_dir() || !temp.join("SKILL.md").is_file() {
            return Err(CoreError::invalid(
                "temp directory is not a valid skill (missing SKILL.md)",
            ));
        }
        self.install_from_dir(temp)
    }

    /// Uninstall a skill by id. Returns whether it existed.
    ///
    /// Built-in skills are protected: an uninstall request for a skill whose
    /// `origin == BuiltIn` is rejected with an error rather than silently
    /// succeeding. For every other origin (workspace / user / installed) and
    /// for ids that aren't in the registry at all, behavior is unchanged from
    /// the underlying [`SkillManager::uninstall`] (returns `Ok(false)` when
    /// nothing was removed).
    ///
    /// _Validates: Requirements R1.3._
    pub fn uninstall(&mut self, id: &str) -> Result<bool> {
        if let Some(skill) = self.manager.registry().get(id) {
            if skill.meta.origin == SkillOrigin::BuiltIn {
                return Err(CoreError::invalid("built-in skill cannot be uninstalled"));
            }
        }
        self.manager.uninstall(id)
    }

    /// The always-resident catalog blurb (Level-1) for the system prompt.
    pub fn catalog_blurb(&self) -> String {
        self.manager.catalog_blurb()
    }

    /// Preview passive activation for a query: the best trigger-matched skill
    /// and its disclosed body, or `None` if nothing matched.
    pub fn preview_activation(&self, query: &str) -> Option<SkillActivationDto> {
        self.manager
            .auto_activate(query)
            .map(|(id, frag)| SkillActivationDto {
                id,
                body: frag.content,
            })
    }

    /// Actively activate a skill by id, returning its disclosed body.
    pub fn activate(&self, id: &str) -> Option<SkillActivationDto> {
        self.manager.activate(id).map(|frag| SkillActivationDto {
            id: id.to_string(),
            body: frag.content,
        })
    }
}

// ============================================================================
// AI Security Review (skill-marketplace task 5)
// ============================================================================
//
// LLM-driven security review of an unpacked skill, run AFTER the static
// scanner. The model receives a fixed Chinese system prompt that locks the
// output format to:
//
//     === ANALYSIS ===
//     <逐文件简述>
//     === VERDICT ===
//     PASS                      // or `FAIL: <一句话原因>`
//
// Tokens stream back through `on_token` (the Tauri command layer wired up in
// task 8 forwards each one as a `skill-ai-review` event); after the stream
// ends, [`parse_verdict`] extracts a typed [`AiReviewResult`].
//
// _Validates: Requirements 4.6, 4.7, 4.8 (and R10.4 once the
// `skill_install_ai_review_*` settings land in task 12)._

/// Maximum characters of `SKILL.md` content embedded in the LLM user prompt.
/// Caps token cost on huge skill READMEs (R4.6).
///
/// Lowered from 8000 → 4000 in the install-flow speedup pass: the model
/// only needs the leading frontmatter + opening prose to reason about
/// risk, and a smaller user prompt drops time-to-first-token noticeably
/// on big skills like `skill-creator`.
const SKILL_MD_USER_PROMPT_CAP: usize = 4000;

/// The marker the system prompt instructs the model to emit before its
/// verdict. [`parse_verdict`] splits on this exact byte sequence
/// (case-sensitive).
const VERDICT_HEADER: &str = "=== VERDICT ===";

/// System prompt seeded into every AI security review run. Locks the model
/// into the strict ANALYSIS / VERDICT contract that [`parse_verdict`] decodes.
pub const AI_SECURITY_REVIEW_SYSTEM_PROMPT: &str = "你是一名软件安全审计员。下面是一个用户即将安装到本地的第三方 Agent Skill 包。\n请严格审查,只关注以下风险:\n1. 是否会执行 shell / 子进程?\n2. 是否访问网络?目的端是哪些?\n3. 是否读取或上传凭证(.env / 环境变量 / keychain)?\n4. 是否会修改 skill 包目录之外的文件?\n5. 是否包含可疑混淆 / base64 编码 / 远程代码执行?\n\n输出格式(严格):\n=== ANALYSIS ===\n<逐文件简述>\n=== VERDICT ===\nPASS  或  FAIL: <一句话原因>\n\n只输出上述结构,不要别的。";

/// Thinking-budget tier for the AI security review.
///
/// Skill audits are structured "yes/no + short rationale" tasks, so the
/// review deliberately caps at `Medium` — `Deep`'s 65K-token reasoning
/// budget is wasted overhead for this use case (per skill-marketplace QA
/// feedback). Encoding the choice as a closed enum makes "Deep for skill
/// review" unrepresentable at the type level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDepth {
    /// Initial pass run automatically when the install dialog opens. Uses
    /// `ThinkingDepth::Simple` (16K reasoning budget) and caps the
    /// response at 2048 tokens total. Optimized for time-to-first-token.
    #[default]
    Initial,
    /// Deeper second pass, triggered when the user explicitly asks for a
    /// re-review (e.g. after a `FAIL` verdict they don't trust). Uses
    /// `ThinkingDepth::Medium` (32K reasoning budget) and caps the
    /// response at 3072 tokens total. Slower but more thorough.
    ReReview,
}

impl ReviewDepth {
    /// Map this tier to the underlying [`deepagent_models::ThinkingDepth`].
    pub fn thinking_depth(self) -> deepagent_models::ThinkingDepth {
        match self {
            ReviewDepth::Initial => deepagent_models::ThinkingDepth::Simple,
            ReviewDepth::ReReview => deepagent_models::ThinkingDepth::Medium,
        }
    }

    /// Hard ceiling on the model's combined reasoning + reply tokens for
    /// this tier. Set explicitly on [`deepagent_models::ResponseRequest::with_max_output_tokens`]
    /// so the model exits early once the verdict has landed instead of
    /// rambling through the full thinking budget the depth would otherwise
    /// allow.
    pub fn max_output_tokens(self) -> u32 {
        match self {
            ReviewDepth::Initial => 2048,
            ReviewDepth::ReReview => 3072,
        }
    }
}

/// Outcome of an AI security review run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiReviewResult {
    /// True iff the model concluded with a `PASS` verdict.
    pub passed: bool,
    /// Full LLM output (analysis + verdict line). Useful for transcript / debugging.
    pub raw_text: String,
    /// When `passed == false`, the one-line reason captured after `FAIL:`.
    /// `None` when passing or when the verdict line couldn't be parsed.
    pub failure_reason: Option<String>,
}

fn category_label(cat: RiskCategory) -> &'static str {
    match cat {
        RiskCategory::Shell => "shell",
        RiskCategory::Execution => "execution",
        RiskCategory::Network => "network",
        RiskCategory::Credential => "credential",
        RiskCategory::Filesystem => "filesystem",
        RiskCategory::Exfiltration => "exfiltration",
    }
}

fn severity_label(sev: RiskSeverity) -> &'static str {
    match sev {
        RiskSeverity::Safe => "safe",
        RiskSeverity::Warning => "warning",
        RiskSeverity::Danger => "danger",
    }
}

/// Build the structured user message sent to the LLM from a [`ScanReport`].
///
/// Keeps the format stable so:
/// 1. token cost is predictable (SKILL.md is capped at
///    [`SKILL_MD_USER_PROMPT_CAP`] characters),
/// 2. transcript replays decode the same way,
/// 3. the model sees a consistent shape across reviews.
pub fn build_review_user_prompt(scan: &ScanReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("Skill name: {}\n\n", scan.name));

    out.push_str(&format!("Files ({}):\n", scan.files.len()));
    for f in &scan.files {
        out.push_str(&format!("- {} ({} bytes)\n", f.name, f.size));
    }
    out.push('\n');

    let md = &scan.skill_md_content;
    let snippet: &str = if md.len() <= SKILL_MD_USER_PROMPT_CAP {
        md.as_str()
    } else {
        // Walk char boundaries so we never split a multi-byte codepoint.
        let cut = md
            .char_indices()
            .take_while(|(i, _)| *i < SKILL_MD_USER_PROMPT_CAP)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        &md[..cut]
    };
    out.push_str(&format!(
        "SKILL.md (first {} chars):\n",
        SKILL_MD_USER_PROMPT_CAP
    ));
    out.push_str(snippet);
    out.push_str("\n\n");

    out.push_str(&format!("Static scan risks ({}):\n", scan.risks.len()));
    for r in &scan.risks {
        let line_suffix = match r.line {
            Some(n) => format!(":{n}"),
            None => String::new(),
        };
        out.push_str(&format!(
            "- [{sev}/{cat}] {file}{line_suffix} — {detail}\n",
            sev = severity_label(r.severity),
            cat = category_label(r.category),
            file = r.file,
            detail = r.detail,
        ));
    }

    out
}

/// Run AI security review on a scan report. Streams token-by-token via the
/// supplied callback (the Tauri command layer in task 8 forwards each token
/// as a `skill-ai-review` event); resolves to the parsed verdict once the
/// stream ends.
///
/// Routes through [`ChatService::run_review_streaming`] (NOT the generic
/// `run_oneshot_streaming` path), which:
/// - **Picks the model** from `skill_install_ai_review_model` if the user
///   set one, otherwise the catalog's chat model. The Deep → Reasoner
///   swap normal chat applies is intentionally bypassed for this task.
/// - **Forces the thinking budget** picked by the supplied [`ReviewDepth`]
///   tier (`Simple` for `Initial`, `Medium` for `ReReview`). The user's
///   global `thinking_depth` setting is ignored — Deep is overkill for a
///   structured PASS/FAIL audit.
/// - **Caps the response** at the tier's `max_output_tokens` so the model
///   exits early after emitting the verdict instead of consuming the full
///   thinking budget.
///
/// _Validates: Requirements 4.6, 4.7, 4.8._
pub async fn ai_security_review<F>(
    chat: &ChatService,
    scan: &ScanReport,
    depth: ReviewDepth,
    on_token: F,
) -> Result<AiReviewResult>
where
    F: FnMut(&str) + Send + 'static,
{
    let user_prompt = build_review_user_prompt(scan);
    let raw_text = chat
        .run_review_streaming(
            AI_SECURITY_REVIEW_SYSTEM_PROMPT,
            &user_prompt,
            depth.thinking_depth(),
            depth.max_output_tokens(),
            on_token,
        )
        .await?;
    Ok(parse_verdict(&raw_text))
}

/// Pure verdict parser: extract the verdict from the raw LLM output.
///
/// The streaming-LLM integration is exercised manually / by the frontend e2e
/// test scheduled for task 24; this parser is the part with branching logic
/// worth unit-testing.
///
/// Logic:
/// 1. Find `=== VERDICT ===` (case-sensitive). If absent → malformed.
/// 2. Take the first non-empty trimmed line after the header.
/// 3. Match (case-insensitive) on the line's prefix:
///    - `PASS…` → `passed = true`, no reason.
///    - `FAIL: …` → `passed = false`, reason = `…` trimmed (or `(reason not provided)` if empty).
///    - `FAIL` (no colon) → `passed = false`, reason = `(reason not provided)`.
///    - anything else → malformed (parse-failure reason carries a 200-char prefix).
pub fn parse_verdict(raw: &str) -> AiReviewResult {
    let raw_text = raw.to_string();
    let after = match raw.split_once(VERDICT_HEADER) {
        Some((_, after)) => after,
        None => return malformed(&raw_text, "verdict line could not be parsed"),
    };
    let line = after
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let upper = line.to_ascii_uppercase();

    if upper.starts_with("PASS") {
        return AiReviewResult {
            passed: true,
            raw_text,
            failure_reason: None,
        };
    }

    // `FAIL:` and `FAIL` are 4/5 ASCII bytes — case toggling preserves byte
    // length, so the byte slice on `line` lines up with the upper-cased prefix.
    if upper.starts_with("FAIL:") {
        let reason = line[5..].trim();
        let reason = if reason.is_empty() {
            "(reason not provided)".to_string()
        } else {
            reason.to_string()
        };
        return AiReviewResult {
            passed: false,
            raw_text,
            failure_reason: Some(reason),
        };
    }

    if upper.starts_with("FAIL") {
        return AiReviewResult {
            passed: false,
            raw_text,
            failure_reason: Some("(reason not provided)".to_string()),
        };
    }

    malformed(&raw_text, "verdict line could not be parsed")
}

fn malformed(raw: &str, why: &str) -> AiReviewResult {
    let prefix: String = raw.chars().take(200).collect();
    AiReviewResult {
        passed: false,
        raw_text: raw.to_string(),
        failure_reason: Some(format!("{why}: {prefix}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn lists_and_previews() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("skills");
        write(
            &ws.join("pdf").join("SKILL.md"),
            "---\nname: PDF\ndescription: Use to \"rotate a pdf\".\n---\nRotate the pdf.",
        );
        let svc = SkillsService::open(Some(ws), tmp.path().join("inst")).unwrap();

        let list = svc.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "pdf");
        assert!(list[0].triggers.contains(&"rotate a pdf".to_string()));

        let preview = svc.preview_activation("please rotate a pdf now").unwrap();
        assert_eq!(preview.id, "pdf");
        assert!(preview.body.contains("Rotate the pdf"));

        // Unrelated query → no activation.
        assert!(svc.preview_activation("what's the weather").is_none());
    }

    #[test]
    fn install_and_uninstall_via_service() {
        let tmp = tempfile::tempdir().unwrap();
        let mut svc = SkillsService::open(None, tmp.path().join("inst")).unwrap();
        assert!(svc.list().is_empty());

        let src = tmp.path().join("commit-helper");
        write(
            &src.join("SKILL.md"),
            "---\nname: Commit Helper\ndescription: \"write a commit\"\n---\nWrite it.",
        );
        let dto = svc.install_from_dir(&src).unwrap();
        assert_eq!(dto.id, "commit-helper");
        assert_eq!(dto.origin, "installed");
        assert_eq!(svc.list().len(), 1);

        assert!(svc.uninstall("commit-helper").unwrap());
        assert!(svc.list().is_empty());
    }

    // ------------------------------------------------------------------
    // install_from_temp + uninstall guard (skill-marketplace task 6).
    // ------------------------------------------------------------------

    /// _Validates: Requirements R3.6._
    #[test]
    fn install_from_temp_validates_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let mut svc = SkillsService::open(None, tmp.path().join("inst")).unwrap();

        // An empty (but real) directory: no SKILL.md → reject.
        let empty = tmp.path().join("empty-temp");
        fs::create_dir_all(&empty).unwrap();
        let err = svc.install_from_temp(&empty).unwrap_err();
        assert!(
            err.to_string().contains("missing SKILL.md"),
            "unexpected error: {err}"
        );
        assert!(svc.list().is_empty());
    }

    /// _Validates: Requirements R3.6._
    #[test]
    fn install_from_temp_happy_path() {
        let tmp = tempfile::tempdir().unwrap();
        let mut svc = SkillsService::open(None, tmp.path().join("inst")).unwrap();

        let temp_skill = tmp.path().join("downloaded").join("pdf-rotate");
        write(
            &temp_skill.join("SKILL.md"),
            "---\nname: PDF Rotate\ndescription: \"rotate a pdf\"\nversion: 0.1.0\n---\nDo the rotate.",
        );

        let dto = svc.install_from_temp(&temp_skill).unwrap();
        assert_eq!(dto.id, "pdf-rotate");
        assert_eq!(dto.origin, "installed");
        assert_eq!(dto.version.as_deref(), Some("0.1.0"));

        let listed = svc.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "pdf-rotate");
        assert_eq!(listed[0].origin, "installed");
    }

    /// _Validates: Requirements R3.6._
    #[test]
    fn install_from_temp_rejects_not_a_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let mut svc = SkillsService::open(None, tmp.path().join("inst")).unwrap();

        // Path that does not exist at all.
        let missing = tmp.path().join("does-not-exist");
        let err = svc.install_from_temp(&missing).unwrap_err();
        assert!(
            err.to_string().contains("missing SKILL.md"),
            "unexpected error: {err}"
        );

        // Path that exists but is a file, not a directory.
        let file_path = tmp.path().join("not-a-dir.txt");
        fs::write(&file_path, "hello").unwrap();
        let err = svc.install_from_temp(&file_path).unwrap_err();
        assert!(
            err.to_string().contains("missing SKILL.md"),
            "unexpected error: {err}"
        );

        assert!(svc.list().is_empty());
    }

    /// _Validates: Requirements R1.3._
    #[test]
    fn uninstall_blocks_builtin() {
        // Programmatically register a BuiltIn skill — the uninstall guard
        // should reject the request and leave the registry untouched.
        let tmp = tempfile::tempdir().unwrap();
        let mut manager = SkillManager::new(None, tmp.path().join("inst"));
        let fm = deepagent_skills::frontmatter::parse(
            "---\nname: Built In One\ndescription: \"do builtin work\"\n---\nbody",
        );
        let skill =
            deepagent_skills::Skill::from_frontmatter("builtin-one", &fm, SkillOrigin::BuiltIn)
                .expect("valid frontmatter");
        manager.register(skill);

        let mut svc = SkillsService::from_manager(manager);
        assert!(svc.list().iter().any(|s| s.id == "builtin-one"));

        let err = svc.uninstall("builtin-one").unwrap_err();
        assert!(
            err.to_string()
                .contains("built-in skill cannot be uninstalled"),
            "unexpected error: {err}"
        );

        // Skill is still in the registry.
        assert!(
            svc.list().iter().any(|s| s.id == "builtin-one"),
            "BuiltIn skill should remain after rejected uninstall"
        );
    }

    /// _Validates: Requirements R1.3, R3.6._
    #[test]
    fn uninstall_allows_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut svc = SkillsService::open(None, tmp.path().join("inst")).unwrap();

        let temp_skill = tmp.path().join("downloaded").join("note-taker");
        write(
            &temp_skill.join("SKILL.md"),
            "---\nname: Note Taker\ndescription: \"take notes\"\n---\nbody",
        );
        let dto = svc.install_from_temp(&temp_skill).unwrap();
        assert_eq!(dto.origin, "installed");
        assert_eq!(svc.list().len(), 1);

        // Uninstall an Installed-origin skill: allowed.
        assert!(svc.uninstall("note-taker").unwrap());
        assert!(svc.list().is_empty());
    }

    /// _Validates: Requirements R1.3._
    #[test]
    fn uninstall_returns_false_for_unknown_id() {
        let tmp = tempfile::tempdir().unwrap();
        let mut svc = SkillsService::open(None, tmp.path().join("inst")).unwrap();

        // Unknown id: the registry lookup misses, the guard doesn't fire,
        // and the underlying manager reports "nothing to remove" via Ok(false).
        let removed = svc.uninstall("nonexistent").unwrap();
        assert!(!removed);
    }

    #[test]
    fn active_activation_by_id() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("skills");
        write(
            &ws.join("fe").join("SKILL.md"),
            "---\nname: FE\ndescription: \"build a dashboard\"\n---\nBuild it well.",
        );
        let svc = SkillsService::open(Some(ws), tmp.path().join("inst")).unwrap();
        let act = svc.activate("fe").unwrap();
        assert!(act.body.contains("Build it well"));
        assert!(svc.activate("missing").is_none());
    }

    #[test]
    fn open_v2_loads_four_roots_with_precedence() {
        let tmp = tempfile::tempdir().unwrap();
        let builtin = tmp.path().join("builtin");
        let user = tmp.path().join("user");
        let marketplace = user.join("marketplace");
        let workspace = tmp.path().join("workspace");

        // Same id "shared" appears in BuiltIn AND User — User wins.
        write(
            &builtin.join("shared").join("SKILL.md"),
            "---\nname: Shared (BuiltIn)\ndescription: \"shared from builtin\"\n---\nBuiltIn body",
        );
        write(
            &user.join("shared").join("SKILL.md"),
            "---\nname: Shared (User)\ndescription: \"shared from user\"\n---\nUser body",
        );

        // Distinct ids — one per origin to verify each root is loaded.
        write(
            &builtin.join("only-builtin").join("SKILL.md"),
            "---\nname: Builtin Only\ndescription: \"only builtin\"\n---\nbody",
        );
        write(
            &marketplace.join("only-installed").join("SKILL.md"),
            "---\nname: Installed Only\ndescription: \"only installed\"\n---\nbody",
        );
        write(
            &user.join("only-user").join("SKILL.md"),
            "---\nname: User Only\ndescription: \"only user\"\n---\nbody",
        );
        write(
            &workspace.join("only-workspace").join("SKILL.md"),
            "---\nname: Workspace Only\ndescription: \"only workspace\"\n---\nbody",
        );

        // And: a Workspace-origin skill that overrides an Installed-origin one.
        write(
            &marketplace.join("override-me").join("SKILL.md"),
            "---\nname: Override (Installed)\ndescription: \"installed version\"\n---\ninstalled",
        );
        write(
            &workspace.join("override-me").join("SKILL.md"),
            "---\nname: Override (Workspace)\ndescription: \"workspace version\"\n---\nworkspace",
        );

        let svc = SkillsService::open_v2(SkillsRoots {
            builtin,
            user,
            marketplace,
            workspace: Some(workspace),
        })
        .unwrap();

        let list = svc.list();
        let by_id: std::collections::HashMap<_, _> =
            list.iter().map(|s| (s.id.clone(), s.clone())).collect();

        // Each origin contributed its distinct skill.
        assert_eq!(by_id.get("only-builtin").unwrap().origin, "built_in");
        assert_eq!(by_id.get("only-installed").unwrap().origin, "installed");
        assert_eq!(by_id.get("only-user").unwrap().origin, "user");
        assert_eq!(by_id.get("only-workspace").unwrap().origin, "workspace");

        // Conflict precedence: User wins over BuiltIn.
        let shared = by_id.get("shared").unwrap();
        assert_eq!(shared.origin, "user");
        assert_eq!(shared.name, "Shared (User)");

        // Conflict precedence: Workspace wins over Installed.
        let overridden = by_id.get("override-me").unwrap();
        assert_eq!(overridden.origin, "workspace");
        assert_eq!(overridden.name, "Override (Workspace)");

        // Total: 6 distinct ids (only-builtin, only-installed, only-user,
        // only-workspace, shared, override-me).
        assert_eq!(list.len(), 6);
    }

    #[test]
    fn open_v2_handles_missing_roots_gracefully() {
        let tmp = tempfile::tempdir().unwrap();
        // builtin and user exist (with one skill each); marketplace and
        // workspace are absent. Should not error.
        let builtin = tmp.path().join("builtin");
        let user = tmp.path().join("user");
        let marketplace = tmp.path().join("nonexistent-marketplace");

        write(
            &builtin.join("b1").join("SKILL.md"),
            "---\nname: B1\ndescription: \"builtin one\"\n---\nbody",
        );
        write(
            &user.join("u1").join("SKILL.md"),
            "---\nname: U1\ndescription: \"user one\"\n---\nbody",
        );

        let svc = SkillsService::open_v2(SkillsRoots {
            builtin,
            user,
            marketplace,
            workspace: None,
        })
        .unwrap();

        let ids: Vec<_> = svc.list().into_iter().map(|s| s.id).collect();
        assert!(ids.contains(&"b1".to_string()));
        assert!(ids.contains(&"u1".to_string()));
        assert_eq!(svc.list().len(), 2);
    }

    #[test]
    fn open_v2_reload_keeps_builtin_and_installed_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let builtin = tmp.path().join("builtin");
        let user = tmp.path().join("user");
        let marketplace = user.join("marketplace");

        write(
            &builtin.join("builtin-skill").join("SKILL.md"),
            "---\nname: Builtin Skill\ndescription: \"builtin\"\n---\nbuiltin body",
        );
        write(
            &user.join("user-skill").join("SKILL.md"),
            "---\nname: User Skill\ndescription: \"user\"\n---\nuser body",
        );
        write(
            &marketplace.join("installed-skill").join("SKILL.md"),
            "---\nname: Installed Skill\ndescription: \"installed\"\n---\ninstalled body",
        );

        let mut svc = SkillsService::open_v2(SkillsRoots {
            builtin,
            user,
            marketplace,
            workspace: None,
        })
        .unwrap();

        assert!(svc.list().iter().any(|s| s.id == "builtin-skill"));
        assert!(svc.list().iter().any(|s| s.id == "user-skill"));
        assert!(svc.list().iter().any(|s| s.id == "installed-skill"));

        svc.reload().unwrap();

        let ids: Vec<_> = svc.list().into_iter().map(|s| s.id).collect();
        assert!(ids.contains(&"builtin-skill".to_string()));
        assert!(ids.contains(&"user-skill".to_string()));
        assert!(ids.contains(&"installed-skill".to_string()));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn open_v2_picks_up_nested_superpowers_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let builtin = tmp.path().join("builtin");

        // Mimic <root>/superpowers/skills/<sub>/SKILL.md (the real superpowers
        // layout discovered at depth 3).
        write(
            &builtin
                .join("superpowers")
                .join("skills")
                .join("debugging")
                .join("SKILL.md"),
            "---\nname: Debugging\ndescription: \"systematic debugging\"\n---\nbody",
        );
        write(
            &builtin
                .join("superpowers")
                .join("skills")
                .join("tdd")
                .join("SKILL.md"),
            "---\nname: TDD\ndescription: \"red green refactor\"\n---\nbody",
        );
        // A flat-layout skill alongside the nested one.
        write(
            &builtin.join("flat").join("SKILL.md"),
            "---\nname: Flat\ndescription: \"flat top level\"\n---\nbody",
        );

        let svc = SkillsService::open_v2(SkillsRoots {
            builtin,
            user: tmp.path().join("user"),
            marketplace: tmp.path().join("marketplace"),
            workspace: None,
        })
        .unwrap();

        let ids: Vec<_> = svc.list().into_iter().map(|s| s.id).collect();
        assert!(ids.contains(&"flat".to_string()));
        assert!(ids.contains(&"debugging".to_string()));
        assert!(ids.contains(&"tdd".to_string()));
        assert_eq!(svc.list().len(), 3);
    }

    // ------------------------------------------------------------------
    // AI Security Review verdict parser (skill-marketplace task 5).
    //
    // The streaming-LLM integration is exercised manually / by the frontend
    // e2e test scheduled for task 24. These tests pin down the parser, which
    // is the part with non-trivial branching.
    // ------------------------------------------------------------------

    // --- ReviewDepth tier mapping --------------------------------------

    /// `Initial` → Simple (16K reasoning budget, 2K reply cap).
    /// Intentionally locks in the "fast first pass" contract so a future
    /// edit can't silently bump it back to Medium.
    #[test]
    fn review_depth_initial_picks_simple_thinking() {
        assert_eq!(
            ReviewDepth::Initial.thinking_depth(),
            deepagent_models::ThinkingDepth::Simple
        );
        assert_eq!(ReviewDepth::Initial.max_output_tokens(), 2048);
    }

    /// `ReReview` → Medium. Used by the explicit re-review path the Tauri
    /// command exposes via `re_review = true`.
    #[test]
    fn review_depth_re_review_picks_medium_thinking() {
        assert_eq!(
            ReviewDepth::ReReview.thinking_depth(),
            deepagent_models::ThinkingDepth::Medium
        );
        assert_eq!(ReviewDepth::ReReview.max_output_tokens(), 3072);
    }

    /// Closed-enum guard: skill review is never allowed to pick Deep,
    /// regardless of which tier the caller asks for. Encoded at the type
    /// level — there's no `ReviewDepth::Deep` variant — so this test is
    /// belt-and-braces against someone adding one without thinking through
    /// the QA feedback that motivated capping at Medium.
    #[test]
    fn review_depth_never_maps_to_deep() {
        for tier in [ReviewDepth::Initial, ReviewDepth::ReReview] {
            assert_ne!(
                tier.thinking_depth(),
                deepagent_models::ThinkingDepth::Deep,
                "ReviewDepth::{tier:?} must not promote to Deep"
            );
        }
    }

    /// Default tier is the cheap initial pass — `ai_security_review`'s
    /// callers can rely on `..Default::default()` to get the snappier path.
    #[test]
    fn review_depth_default_is_initial() {
        assert_eq!(ReviewDepth::default(), ReviewDepth::Initial);
    }

    /// _Validates: Requirements 4.8._
    #[test]
    fn parse_verdict_pass() {
        let r = parse_verdict("=== ANALYSIS ===\nlooks fine\n=== VERDICT ===\nPASS\n");
        assert!(r.passed);
        assert!(r.failure_reason.is_none());
    }

    /// _Validates: Requirements 4.8._
    #[test]
    fn parse_verdict_pass_trailing_text() {
        // Trailing whitespace after PASS must still be a pass.
        let r = parse_verdict("=== VERDICT ===\nPASS  \n");
        assert!(r.passed);
        assert!(r.failure_reason.is_none());
    }

    /// _Validates: Requirements 4.8._
    #[test]
    fn parse_verdict_fail_with_reason() {
        let r = parse_verdict("=== VERDICT ===\nFAIL: reads OS keyring without consent\n");
        assert!(!r.passed);
        assert_eq!(
            r.failure_reason.as_deref(),
            Some("reads OS keyring without consent")
        );
    }

    /// _Validates: Requirements 4.8._
    #[test]
    fn parse_verdict_fail_no_colon() {
        let r = parse_verdict("=== VERDICT ===\nFAIL\n");
        assert!(!r.passed);
        assert_eq!(r.failure_reason.as_deref(), Some("(reason not provided)"));
    }

    /// _Validates: Requirements 4.8._
    #[test]
    fn parse_verdict_missing_section() {
        let r = parse_verdict("=== ANALYSIS ===\nno verdict block at all");
        assert!(!r.passed);
        let reason = r.failure_reason.expect("malformed → reason set");
        assert!(
            reason.contains("verdict line could not be parsed"),
            "reason: {reason}"
        );
    }

    /// _Validates: Requirements 4.8._
    #[test]
    fn parse_verdict_malformed() {
        // Header present but the line that follows isn't PASS/FAIL.
        let r = parse_verdict("=== VERDICT ===\nMaybe okay?\n");
        assert!(!r.passed);
        let reason = r.failure_reason.expect("malformed → reason set");
        assert!(
            reason.contains("verdict line could not be parsed"),
            "reason: {reason}"
        );
    }

    /// _Validates: Requirements 4.8._
    ///
    /// The verdict line check is intentionally case-insensitive on `PASS` /
    /// `FAIL` (the system prompt asks for upper-case but local LLMs sometimes
    /// downcase tokens, and we'd rather accept than mis-flag a pass). The
    /// `=== VERDICT ===` header itself stays case-sensitive — same wording
    /// as the system prompt.
    #[test]
    fn parse_verdict_case_insensitive() {
        let r = parse_verdict("=== VERDICT ===\npass\n");
        assert!(r.passed);
        assert!(r.failure_reason.is_none());

        let r2 = parse_verdict("=== VERDICT ===\nfail: bad\n");
        assert!(!r2.passed);
        assert_eq!(r2.failure_reason.as_deref(), Some("bad"));
    }

    /// _Validates: Requirements 4.8._
    #[test]
    fn parse_verdict_with_analysis_block() {
        // A realistic, multi-line ANALYSIS section followed by the VERDICT.
        let raw = "=== ANALYSIS ===\n\
            - SKILL.md: well-documented planning helper, no execution.\n\
            - scripts/run.py: subprocess.run with shell=False (safe).\n\
            - data/notes.md: prose only.\n\
            \n\
            === VERDICT ===\n\
            PASS\n";
        let r = parse_verdict(raw);
        assert!(r.passed, "raw_text: {}", r.raw_text);
        assert!(r.failure_reason.is_none());
        // The full output is preserved on the result for transcript / debugging.
        assert!(r.raw_text.contains("subprocess.run"));
    }
}
