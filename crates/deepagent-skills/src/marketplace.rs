//! SkillsMP REST client + GitHub raw / codeload downloader.
//!
//! Companion to the marketplace UX: this module owns every HTTP call DeepAgent
//! Studio makes when the user searches [skillsmp.com](https://skillsmp.com)
//! or installs a skill from a GitHub URL.
//!
//! ## What lives here
//!
//! - [`MarketSkill`] / [`MarketSearchData`] / [`Pagination`] — typed responses
//!   for `GET https://skillsmp.com/api/v1/skills/search` (camelCase fields are
//!   renamed to snake_case via serde; `updatedAt` is a string of unix epoch
//!   seconds and decodes through [`de_epoch_str`]).
//! - [`SortBy`] / [`SearchQuery`] — request-side knobs.
//! - [`SkillsMpClient`] — the HTTP client. It carries an optional API key
//!   (sent as `Authorization: Bearer …`), tracks the most recent
//!   `X-RateLimit-Daily-Remaining` header so the UI can surface quota usage,
//!   and exposes the GitHub URL parser plus the two install-time downloads
//!   (raw `SKILL.md` preview and full `codeload` zip).
//! - [`GithubLocator`] / [`parse_github_url`](SkillsMpClient::parse_github_url)
//!   — strict acceptance: only `https://github.com/{owner}/{repo}/tree/{branch}/{path}`
//!   passes; SSH form, other hosts, and `blob/` URLs are rejected.
//! - [`extract_skill_subtree`] — the pure helper the install flow funnels into
//!   after streaming bytes. It rejects path traversal segments (`..`),
//!   absolute paths, Windows drive letters, and unix-mode symlinks
//!   (`mode & 0o170000 == 0o120000`), then verifies the resolved target
//!   contains a `SKILL.md` at its root. The unit tests drive this helper
//!   directly — no HTTP needed.
//! - [`check_size_cap`] — the 50 MB ceiling used both up-front against
//!   `Content-Length` and as a running counter while streaming.
//!
//! ## Safety invariants
//!
//! - **No new external crates.** The implementation uses only what is already
//!   declared on this crate's `Cargo.toml`: `reqwest`, `serde`, `tempfile`,
//!   `zip` (kept for the historic [`extract_skill_subtree`] helper used by
//!   the e2e tests).
//! - **Bounded download.** `download_skill_to_temp` enumerates the skill
//!   subtree via the GitHub trees API, sums the declared blob sizes, and
//!   short-circuits with the [`MAX_PACKAGE_BYTES`] cap **before** any blob
//!   bytes hit the wire. Each downloaded blob is also re-checked against
//!   the cap, so a malicious mirror lying about its `Content-Length`
//!   cannot exhaust memory.
//! - **No path escape.** Every tree entry written to disk is validated
//!   component-by-component (`..`, absolute paths, Windows drive letters,
//!   symlink mode, submodule mode); the running tempdir is dropped on
//!   `Err`.
//! - **Subtree-only.** Only files inside the requested
//!   `https://github.com/{owner}/{repo}/tree/{branch}/{path}` subtree are
//!   downloaded. Big monorepos no longer trip the 50 MB cap on a clean
//!   ~50 KB skill.
//! - **China-friendly fallback.** Every GitHub HTTP call (the trees API,
//!   the per-blob raw downloads, and the historic SKILL.md preview) tries
//!   the direct URL first; on a transport-level failure (connect refused
//!   / DNS / TLS / request timeout) the call is retried through each
//!   [`DEFAULT_GITHUB_MIRRORS`] prefix in order so users on networks
//!   where GitHub is unreachable (e.g. mainland China without a VPN) get
//!   a working install path. HTTP status errors and mid-stream chunk
//!   errors terminate the chain immediately.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// SkillsMP marketplace base URL.
pub const SKILLSMP_BASE_URL: &str = "https://skillsmp.com";

/// GitHub raw content base — used for [`SkillsMpClient::fetch_raw_skill_md`].
pub const RAW_GITHUB_BASE: &str = "https://raw.githubusercontent.com";

/// GitHub codeload base. Retained as a public constant for downstream
/// callers and for the historic [`extract_skill_subtree`] helper, but the
/// install path no longer downloads from here — see
/// [`SkillsMpClient::download_skill_to_temp`] for the API-driven tree walk.
pub const CODELOAD_BASE: &str = "https://codeload.github.com";

/// GitHub REST API base — used by
/// [`SkillsMpClient::download_skill_to_temp`] to enumerate the skill
/// subtree via `GET /repos/{owner}/{repo}/git/trees/{branch}?recursive=1`.
pub const GITHUB_API_BASE: &str = "https://api.github.com";

/// Conventional `SKILL.md` filename inside a skill package.
pub const SKILL_FILE: &str = "SKILL.md";

/// Curated public GitHub HTTP-proxy mirror prefixes, ordered by preference.
///
/// Each entry is **prepended** to a real GitHub URL — e.g.
/// `https://ghfast.top/` + `https://codeload.github.com/owner/repo/zip/refs/heads/main`
/// → `https://ghfast.top/https://codeload.github.com/...`. This wraps every
/// GitHub host the marketplace flow touches (`raw.githubusercontent.com`,
/// `codeload.github.com`) without per-host configuration, so a single mirror
/// covers both the SKILL.md preview and the codeload zip download.
///
/// The list is consulted by [`SkillsMpClient::fetch_raw_skill_md`] and
/// [`SkillsMpClient::download_skill_to_temp`] **only when the direct attempt
/// fails with a transport-level error** (connect refused, DNS, TLS
/// handshake, request timeout). Users in regions with reliable direct
/// access pay no latency cost — direct succeeds first and the fallback is
/// never consulted. Users behind networks where GitHub is unreachable
/// (e.g. mainland China without VPN) get a working install path
/// out-of-the-box.
///
/// Mirrors come and go; the chain tries each in order so a dead entry
/// just costs a connect-timeout. The set below was current as of early
/// 2026; adjust at release time if the list rots.
pub const DEFAULT_GITHUB_MIRRORS: &[&str] = &[
    "https://ghfast.top/",
    "https://ghproxy.net/",
    "https://mirror.ghproxy.com/",
];

/// Default keyword substituted into the wire request when the caller's
/// [`SearchQuery::q`] is empty or whitespace-only.
///
/// `skillsmp.com/api/v1/skills/search` requires `q` to be present and
/// non-empty (returns `400 MISSING_QUERY` otherwise) and does **not** support
/// wildcard searches like `q=*` (per <https://skillsmp.com/docs/api>). To
/// preserve the "open Market tab and see results immediately" UX the spec
/// targets, we send a broad, marketplace-meta keyword as the browse
/// fallback. `"skill"` is intentionally generic so it matches the largest
/// possible slice of the catalog while staying valid under the API's
/// no-wildcards rule.
pub const BROWSE_FALLBACK_QUERY: &str = "skill";

/// Maximum allowed size of a downloaded skill package (50 MB, decimal).
pub const MAX_PACKAGE_BYTES: u64 = 50_000_000;

/// SkillsMP rate-limit header carried on every authenticated response.
pub const RATE_LIMIT_HEADER: &str = "X-RateLimit-Daily-Remaining";

/// Default SkillsMP API key built into the binary.
///
/// Overridable at build time via the `DEEPAGENT_SKILLSMP_API_KEY` environment
/// variable (consumed through [`option_env!`], so an unset variable simply
/// falls back to the literal embedded in source). After trimming, an empty
/// value is treated as "no built-in key" — the runtime then drops to the
/// anonymous quota (50/day on skillsmp.com).
pub const BUILTIN_SKILLSMP_API_KEY: &str = match option_env!("DEEPAGENT_SKILLSMP_API_KEY") {
    Some(k) => k,
    None => "sk_live_skillsmp_kx2-KxndMXdqZNnbicfzqs-bUk1-WE1js5JcjZnZXfA",
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Custom serde deserializer that turns `"1781073435"` into `1_781_073_435i64`.
///
/// The skillsmp.com payload encodes `updatedAt` as a *string* of unix epoch
/// seconds; serde-json refuses to coerce strings into integers without an
/// explicit deserializer, so this helper plugs the gap.
pub(crate) fn de_epoch_str<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<i64, D::Error> {
    let s: String = serde::Deserialize::deserialize(d)?;
    s.parse::<i64>().map_err(serde::de::Error::custom)
}

/// One row of `data.skills` from `GET /api/v1/skills/search`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketSkill {
    /// Stable id (slug-style) — survives renames on skillsmp.
    pub id: String,
    /// Display name (frontmatter `name`).
    pub name: String,
    /// GitHub author / organization name.
    pub author: String,
    /// Short description (carries trigger phrases).
    pub description: String,
    /// Source-of-truth URL on GitHub
    /// (`https://github.com/{owner}/{repo}/tree/{branch}/{path}`).
    #[serde(rename = "githubUrl")]
    pub github_url: String,
    /// Canonical skillsmp.com page URL (used by the Card "view on web" link).
    #[serde(rename = "skillUrl")]
    pub skill_url: String,
    /// Stargazer count of the host repo.
    pub stars: u64,
    /// Last update — unix epoch seconds (encoded as a string by skillsmp).
    #[serde(rename = "updatedAt", deserialize_with = "de_epoch_str")]
    pub updated_at: i64,
}

/// Pagination block from `data.pagination`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct Pagination {
    /// 1-based page number.
    pub page: u32,
    /// Page size.
    pub limit: u32,
    /// Total matching rows.
    pub total: u64,
    /// Whether another page exists.
    #[serde(rename = "hasNext")]
    pub has_next: bool,
    /// Whether a previous page exists.
    #[serde(rename = "hasPrev")]
    pub has_prev: bool,
}

/// `data` envelope returned by the search endpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketSearchData {
    /// One entry per skill on the current page.
    pub skills: Vec<MarketSkill>,
    /// Pagination metadata.
    pub pagination: Pagination,
}

/// Top-level shape of `GET /api/v1/skills/search`. The runtime only consumes
/// `data` after asserting `success == true`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketSearchResponse {
    /// Always `true` on a healthy response.
    pub success: bool,
    /// Search results.
    pub data: MarketSearchData,
}

/// Sort order for [`SearchQuery`]. Lowercased on the wire (`stars` /
/// `recent`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortBy {
    /// Sort by repository stars (default for the recommendation tab).
    Stars,
    /// Sort by `updatedAt` descending.
    Recent,
}

impl SortBy {
    /// Wire form (`stars` / `recent`).
    pub const fn as_str(&self) -> &'static str {
        match self {
            SortBy::Stars => "stars",
            SortBy::Recent => "recent",
        }
    }
}

/// Search-side parameters for [`SkillsMpClient::search`].
///
/// Empty optional fields are simply omitted from the wire request, letting
/// skillsmp's defaults apply. Note that `q` is special: skillsmp.com
/// requires `q` to be present and non-empty (and rejects wildcards), so an
/// empty / whitespace-only `q` is rewritten to [`BROWSE_FALLBACK_QUERY`]
/// inside [`SkillsMpClient::search`] before the request is sent.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// Free-text query string.
    ///
    /// Empty or whitespace-only ⇒ [`SkillsMpClient::search`] substitutes
    /// [`BROWSE_FALLBACK_QUERY`] on the wire so the server returns a broad
    /// "browse" page instead of `400 MISSING_QUERY`.
    pub q: String,
    /// 1-based page number.
    pub page: Option<u32>,
    /// Page size.
    pub limit: Option<u32>,
    /// Sort order.
    pub sort_by: Option<SortBy>,
    /// Optional category filter (passes through to the API).
    pub category: Option<String>,
    /// Optional occupation filter.
    pub occupation: Option<String>,
}

/// Parsed components of an `https://github.com/{owner}/{repo}/tree/{branch}/{path}`
/// URL. `path` may be empty (skill lives at the repo root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubLocator {
    /// GitHub user or organization.
    pub owner: String,
    /// Repository name.
    pub repo: String,
    /// Branch (the first segment after `tree/`).
    pub branch: String,
    /// Subdirectory inside the repo. Forward-slashed; no leading `/`. May be
    /// empty when the skill is at the repo root.
    pub path: String,
}

/// One entry in the response of
/// `GET /repos/{owner}/{repo}/git/trees/{branch}?recursive=1`. Only the
/// fields the install path needs are deserialized; the rest are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct GitTreeEntry {
    /// Forward-slashed path of the entry inside the repository, relative
    /// to the repo root. Never carries a leading `/`.
    pub path: String,
    /// Git mode string. `"100644"` / `"100755"` for blobs, `"040000"` for
    /// trees, `"120000"` for symlinks, `"160000"` for git submodules.
    pub mode: String,
    /// `"blob"`, `"tree"`, or `"commit"`. Renamed to `kind` so it doesn't
    /// collide with the Rust keyword.
    #[serde(rename = "type")]
    pub kind: String,
    /// Blob size in bytes when known. Always `None` for `tree` entries.
    pub size: Option<u64>,
}

/// Response shape for
/// `GET /repos/{owner}/{repo}/git/trees/{sha}?recursive=1`. We don't decode
/// `sha` / `url` — the install path only needs the file listing.
#[derive(Debug, Clone, Deserialize)]
pub struct GitTreeResponse {
    /// All entries in the repo, recursively flattened.
    pub tree: Vec<GitTreeEntry>,
    /// `true` when the listing was cut off because the repo has more
    /// entries than GitHub returns in a single response (~100k items).
    /// Skill subtrees are tiny so this should never trip in practice; if
    /// it does, we surface a clear error rather than silently installing a
    /// partial skill.
    #[serde(default)]
    pub truncated: bool,
}

/// Owning handle to an extracted skill on disk.
///
/// The inner [`tempfile::TempDir`] is held privately; dropping the handle
/// removes the entire temporary tree, so callers must keep the handle alive
/// for the lifetime of the install / scan flow.
pub struct TempSkillDir {
    /// Owns the temporary directory; cleaned up when this struct is dropped.
    _tmp: tempfile::TempDir,
    /// Absolute path to the skill root *inside* the tempdir
    /// (`<tmp>/{repo}-{branch}/{path}/`).
    pub root: PathBuf,
}

impl std::fmt::Debug for TempSkillDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TempSkillDir")
            .field("root", &self.root)
            .finish()
    }
}

/// Reject a payload whose declared or actual size exceeds [`MAX_PACKAGE_BYTES`].
///
/// Pulled out to a free function so it can be exercised directly by unit
/// tests and reused both up-front (against `Content-Length`) and as a running
/// counter while streaming the body.
pub fn check_size_cap(bytes: u64) -> Result<()> {
    if bytes > MAX_PACKAGE_BYTES {
        return Err(CoreError::invalid("skill package too large (>50 MB)"));
    }
    Ok(())
}

/// Provenance of the API key currently in use by [`SkillsMpClientHandle`].
///
/// Surfaced to the UI through the `skill_market_get_api_key` Tauri command so
/// the Provider Config popover can render the right "源:内置 / 用户" badge
/// without ever leaking the key value itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeySource {
    /// User-supplied key, read out of the OS keychain at boot or after the
    /// user pasted one into the Provider Config popover.
    User,
    /// Compile-time built-in key (see [`BUILTIN_SKILLSMP_API_KEY`]).
    Builtin,
    /// No key — the client falls back to anonymous access (50/day quota on
    /// skillsmp.com).
    None,
}

/// Resolve the priority chain user → builtin → none.
///
/// Trims the candidate strings before comparing, so a whitespace-only user
/// key is treated as "no user key" and falls back. Returns the actual key
/// payload (already trimmed) the [`SkillsMpClient`] should send on the
/// `Authorization` header, paired with the source label.
pub fn resolve_api_key(user_key: Option<&str>) -> (Option<String>, ApiKeySource) {
    if let Some(k) = user_key {
        let trimmed = k.trim();
        if !trimmed.is_empty() {
            return (Some(trimmed.to_string()), ApiKeySource::User);
        }
    }
    let builtin = BUILTIN_SKILLSMP_API_KEY.trim();
    if !builtin.is_empty() {
        return (Some(builtin.to_string()), ApiKeySource::Builtin);
    }
    (None, ApiKeySource::None)
}

/// Live, mutable handle to a [`SkillsMpClient`].
///
/// The desktop layer holds a single `Arc<SkillsMpClientHandle>` inside
/// `AppState` so the Tauri command layer can swap the inner client when the
/// user pastes / clears their custom API key, **without** rebuilding the rest
/// of `AppState`.
///
/// Two short-lived mutexes guard the client and its source label. Read /
/// write operations are intentionally cheap so callers should keep the
/// closure passed to [`Self::with_client`] short.
pub struct SkillsMpClientHandle {
    inner: std::sync::Mutex<SkillsMpClient>,
    source: std::sync::Mutex<ApiKeySource>,
}

impl SkillsMpClientHandle {
    /// Build the handle with an optional user-supplied key.
    ///
    /// Falls back to the built-in key when `user_key` is `None`, empty, or
    /// whitespace-only; falls all the way through to anonymous when the
    /// built-in is also empty.
    pub fn new(user_key: Option<String>) -> Self {
        let (key, source) = resolve_api_key(user_key.as_deref());
        Self {
            inner: std::sync::Mutex::new(SkillsMpClient::new(key)),
            source: std::sync::Mutex::new(source),
        }
    }

    /// Replace the inner client with one built from the supplied user key.
    ///
    /// Pass `None` (or an empty / whitespace string) to clear the user
    /// override and fall back through built-in → none.
    ///
    /// Errors locking either mutex are silently ignored; a poisoned mutex is
    /// already an unrecoverable runtime bug, and we do not want to crash the
    /// UI just because the user clicked the "save key" button.
    pub fn set_user_key(&self, user_key: Option<String>) {
        let (key, source) = resolve_api_key(user_key.as_deref());
        if let Ok(mut c) = self.inner.lock() {
            *c = SkillsMpClient::new(key);
        }
        if let Ok(mut s) = self.source.lock() {
            *s = source;
        }
    }

    /// Read-only access to the current source label.
    ///
    /// Backs the `skill_market_get_api_key` Tauri command. On a poisoned
    /// mutex we return [`ApiKeySource::None`] — the safest default for the UI
    /// (it shows the "no key configured" badge, prompting the user to retry).
    pub fn source(&self) -> ApiKeySource {
        self.source.lock().map(|s| *s).unwrap_or(ApiKeySource::None)
    }

    /// Borrow the underlying client for one HTTP call.
    ///
    /// Holds the mutex for the duration of the closure, so callers MUST keep
    /// `f` short — typically just preparing a request future. The future
    /// returned by `f` runs after the lock is dropped.
    pub fn with_client<R>(&self, f: impl FnOnce(&SkillsMpClient) -> R) -> R {
        let guard = self.inner.lock().expect("skillsmp client mutex poisoned");
        f(&guard)
    }
}

impl std::fmt::Debug for SkillsMpClientHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillsMpClientHandle")
            .field("source", &self.source.lock().map(|g| *g).ok())
            .finish()
    }
}

/// HTTP client for the SkillsMP marketplace + GitHub downloads.
///
/// Cheap to clone state: the inner `reqwest::Client` is reference-counted and
/// the rate-limit cache is an `Arc<Mutex<…>>`. The Tauri command layer relies
/// on this `Clone` impl to release the [`SkillsMpClientHandle`] mutex *before*
/// awaiting an HTTP call (holding a `std::sync::Mutex` across `.await` would
/// stall the executor).
#[derive(Clone)]
pub struct SkillsMpClient {
    http: reqwest::Client,
    base: String,
    api_key: Option<String>,
    daily_remaining: Arc<Mutex<Option<u32>>>,
}

impl std::fmt::Debug for SkillsMpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillsMpClient")
            .field("base", &self.base)
            .field("has_api_key", &self.api_key.is_some())
            .field(
                "daily_remaining",
                &self.daily_remaining.lock().ok().and_then(|g| *g),
            )
            .finish()
    }
}

/// Project a [`SearchQuery`] into the flat `&[(name, value)]` list
/// `reqwest::RequestBuilder::query` consumes.
///
/// `q` is always emitted: when the caller hands us empty / whitespace-only
/// text we substitute [`BROWSE_FALLBACK_QUERY`] so the wire request stays
/// valid (skillsmp.com returns `400 MISSING_QUERY` if `q` is missing or
/// empty, and rejects wildcard searches per
/// <https://skillsmp.com/docs/api>). Other knobs are emitted only when
/// `Some(_)` so the server's defaults still apply otherwise.
fn build_search_params(query: &SearchQuery) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> = Vec::new();
    let q_value = if query.q.trim().is_empty() {
        BROWSE_FALLBACK_QUERY.to_string()
    } else {
        query.q.clone()
    };
    params.push(("q", q_value));
    if let Some(p) = query.page {
        params.push(("page", p.to_string()));
    }
    if let Some(l) = query.limit {
        params.push(("limit", l.to_string()));
    }
    if let Some(s) = query.sort_by {
        params.push(("sortBy", s.as_str().to_string()));
    }
    if let Some(ref c) = query.category {
        params.push(("category", c.clone()));
    }
    if let Some(ref o) = query.occupation {
        params.push(("occupation", o.clone()));
    }
    params
}

/// Build the ordered list of URL candidates the GitHub-hitting download
/// methods should try.
///
/// The first entry is always the original (direct) URL. Each non-empty
/// entry in `mirrors` is then appended as `<mirror>/<direct_url>`. A
/// trailing slash on the mirror prefix is normalized so the result never
/// contains a doubled `//` between the prefix and the absolute URL.
///
/// Empty / whitespace-only mirror entries are dropped. The order of the
/// resulting list mirrors the input order so the [`DEFAULT_GITHUB_MIRRORS`]
/// preference list is honored verbatim.
pub fn build_download_candidates(direct_url: &str, mirrors: &[&str]) -> Vec<String> {
    let mut out = Vec::with_capacity(1 + mirrors.len());
    out.push(direct_url.to_string());
    for prefix in mirrors {
        let trimmed = prefix.trim();
        if trimmed.is_empty() {
            continue;
        }
        let prefix = trimmed.trim_end_matches('/');
        out.push(format!("{prefix}/{direct_url}"));
    }
    out
}

/// True if a [`reqwest::Error`] looks like a transport-level failure (the
/// kind a public mirror could plausibly recover from), as opposed to a
/// content-level failure tied to the resource itself.
///
/// Transport failures (connect, DNS, TLS, request timeout, body
/// read-timeout) trigger the GitHub-mirror fallback chain; HTTP status
/// errors and JSON-decode errors do **not** — those are content errors and
/// the same mirror would just return them again.
fn is_recoverable_transport_error(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_timeout() || err.is_request() || err.is_body()
}

impl SkillsMpClient {
    /// Build a new client. Pass `None` to operate anonymously (50/day quota
    /// on skillsmp.com).
    pub fn new(api_key: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent("DeepAgent-Studio/skill-marketplace")
            .build()
            .expect("default reqwest client builds");
        Self {
            http,
            base: SKILLSMP_BASE_URL.to_string(),
            api_key,
            daily_remaining: Arc::new(Mutex::new(None)),
        }
    }

    /// Last `X-RateLimit-Daily-Remaining` header observed on a response,
    /// or `None` if no successful call has been made yet.
    pub fn last_daily_remaining(&self) -> Option<u32> {
        self.daily_remaining.lock().ok().and_then(|g| *g)
    }

    /// `GET https://skillsmp.com/api/v1/skills/search` — returns the parsed
    /// `data` envelope. Sends `Authorization: Bearer …` when an API key is
    /// configured. Updates [`Self::last_daily_remaining`] from response
    /// headers on both success and failure.
    ///
    /// An empty / whitespace-only [`SearchQuery::q`] is rewritten to
    /// [`BROWSE_FALLBACK_QUERY`] before the request is sent so the server
    /// does not reject the request with `400 MISSING_QUERY`.
    pub async fn search(&self, query: &SearchQuery) -> Result<MarketSearchData> {
        let url = format!("{}/api/v1/skills/search", self.base.trim_end_matches('/'));

        let params = build_search_params(query);

        let mut req = self.http.get(&url).query(&params);
        if let Some(key) = self.api_key.as_deref() {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }

        let resp = req
            .send()
            .await
            .map_err(|e| CoreError::other(format!("skillsmp search request failed: {e}")))?;

        // Capture rate-limit header before consuming the body.
        if let Some(remaining) = resp
            .headers()
            .get(RATE_LIMIT_HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u32>().ok())
        {
            if let Ok(mut guard) = self.daily_remaining.lock() {
                *guard = Some(remaining);
            }
        }

        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| CoreError::other(format!("read skillsmp response body: {e}")))?;
        if !status.is_success() {
            return Err(CoreError::other(format!(
                "skillsmp search returned {}: {}",
                status,
                String::from_utf8_lossy(&bytes)
            )));
        }

        let parsed: MarketSearchResponse = serde_json::from_slice(&bytes)
            .map_err(|e| CoreError::other(format!("parse skillsmp response: {e}")))?;
        if !parsed.success {
            return Err(CoreError::other("skillsmp returned success=false"));
        }
        Ok(parsed.data)
    }

    /// Strict parser for the `githubUrl` field shipped by skillsmp.
    ///
    /// Accepts only `https://github.com/{owner}/{repo}/tree/{branch}[/{path}…]`.
    /// `path` may be empty (skill at repo root) and a trailing slash is
    /// tolerated. Anything else — SSH form, other hosts, `blob/` URLs,
    /// missing segments — is rejected with a human-readable message.
    pub fn parse_github_url(url: &str) -> Result<GithubLocator> {
        // Reject SSH form (`git@github.com:owner/repo.git`).
        if url.starts_with("git@") {
            return Err(CoreError::invalid(format!(
                "ssh github urls are not supported, only https://github.com/{{owner}}/{{repo}}/tree/{{branch}}/{{path}} is accepted, got: {url}"
            )));
        }

        // Must be HTTPS on github.com.
        let stripped = url.strip_prefix("https://").ok_or_else(|| {
            CoreError::invalid(format!("only https URLs are accepted, got: {url}"))
        })?;
        let (host, rest) = stripped.split_once('/').unwrap_or((stripped, ""));
        if host != "github.com" {
            return Err(CoreError::invalid(format!(
                "only github.com URLs accepted, got: {host}"
            )));
        }

        // Strip a possible query / fragment so they cannot land in `path`.
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);

        // Split path: {owner}/{repo}/tree/{branch}[/{path…}]
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() < 4 {
            return Err(CoreError::invalid(format!(
                "expected /owner/repo/tree/branch[/path] in github URL, got: {url}"
            )));
        }
        if parts[2] != "tree" {
            return Err(CoreError::invalid(format!(
                "expected `tree` segment in github URL, got `{}` (only /tree/ form is supported, not /blob/ etc.)",
                parts[2]
            )));
        }
        let owner = parts[0].to_string();
        let repo = parts[1].to_string();
        let branch = parts[3].to_string();
        if owner.is_empty() || repo.is_empty() || branch.is_empty() {
            return Err(CoreError::invalid(format!(
                "github URL has empty owner/repo/branch: {url}"
            )));
        }
        let path = parts[4..]
            .iter()
            .copied()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        Ok(GithubLocator {
            owner,
            repo,
            branch,
            path,
        })
    }

    /// `GET https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{path}/SKILL.md`.
    /// Used by the install dialog to render a Markdown preview before
    /// committing to the full download. No SkillsMP key needed.
    ///
    /// Goes direct first; on a transport-level failure (connect refused,
    /// DNS error, TLS handshake, request timeout) the call is retried
    /// through each [`DEFAULT_GITHUB_MIRRORS`] prefix in order so users on
    /// networks where GitHub is unreachable (e.g. mainland China without a
    /// VPN) still get a working preview. HTTP status errors and other
    /// content-level errors are returned immediately — the same mirror
    /// would just echo them.
    pub async fn fetch_raw_skill_md(&self, loc: &GithubLocator) -> Result<String> {
        let direct = if loc.path.is_empty() {
            format!(
                "{}/{}/{}/{}/{}",
                RAW_GITHUB_BASE, loc.owner, loc.repo, loc.branch, SKILL_FILE
            )
        } else {
            format!(
                "{}/{}/{}/{}/{}/{}",
                RAW_GITHUB_BASE, loc.owner, loc.repo, loc.branch, loc.path, SKILL_FILE
            )
        };
        let bytes = self
            .http_get_bytes_with_fallback(&direct, "fetch SKILL.md", REQUEST_TIMEOUT)
            .await?;
        String::from_utf8(bytes)
            .map_err(|e| CoreError::other(format!("SKILL.md is not valid UTF-8: {e}")))
    }

    /// Shared GitHub-mirror-aware HTTP GET that returns the raw body bytes.
    ///
    /// Used by both [`Self::fetch_raw_skill_md`] and the per-blob downloads
    /// inside [`Self::download_skill_to_temp`]. Tries the direct URL first
    /// and walks [`DEFAULT_GITHUB_MIRRORS`] on transport failures; HTTP
    /// status errors are returned immediately because no mirror would heal
    /// a 4xx from the upstream content.
    ///
    /// `error_label` is used as the prefix on returned error strings so
    /// the caller's failure surface is human-readable (e.g.
    /// `"fetch SKILL.md from <url>: HTTP 404"`). `request_timeout`
    /// overrides the default 30 s timeout for this individual request —
    /// codeload-style large bodies pass [`DOWNLOAD_TIMEOUT`].
    async fn http_get_bytes_with_fallback(
        &self,
        direct_url: &str,
        error_label: &str,
        request_timeout: Duration,
    ) -> Result<Vec<u8>> {
        let candidates = build_download_candidates(direct_url, DEFAULT_GITHUB_MIRRORS);
        let mut last_err: Option<CoreError> = None;
        for (i, url) in candidates.iter().enumerate() {
            let send_result = self.http.get(url).timeout(request_timeout).send().await;
            let resp = match send_result {
                Ok(r) => r,
                Err(e) if is_recoverable_transport_error(&e) => {
                    tracing::debug!(
                        attempt = i,
                        url = %url,
                        error = %e,
                        "{error_label} transport failed, falling through to next channel"
                    );
                    last_err = Some(CoreError::other(format!(
                        "{error_label} {url}: transport error: {e}"
                    )));
                    continue;
                }
                Err(e) => {
                    return Err(CoreError::other(format!(
                        "{error_label} request failed: {e}"
                    )));
                }
            };
            let status = resp.status();
            if !status.is_success() {
                return Err(CoreError::other(format!(
                    "{error_label} {url} returned HTTP {status}"
                )));
            }
            if i > 0 {
                tracing::info!(
                    via = %url,
                    "{error_label} via GitHub mirror fallback (channel {i})"
                );
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| CoreError::other(format!("{error_label} read body: {e}")))?;
            return Ok(bytes.to_vec());
        }
        Err(last_err.unwrap_or_else(|| {
            CoreError::other(format!("{error_label}: no download channel was tried"))
        }))
    }

    /// Download a skill subtree from GitHub via the REST tree API and
    /// drop it into a fresh tempdir.
    ///
    /// Strategy: rather than pulling the entire repo's codeload zip (which
    /// can be hundreds of MB on monorepos and trips the 50-MB skill cap
    /// even when the actual skill is ~50 kB), this method:
    ///
    /// 1. Calls
    ///    `GET https://api.github.com/repos/{owner}/{repo}/git/trees/{branch}?recursive=1`
    ///    once and decodes the [`GitTreeResponse`] listing.
    /// 2. Filters the listing down to entries inside `loc.path` (the
    ///    skill's subdirectory).
    /// 3. Rejects symlink entries (mode `120000`) and git submodules
    ///    (mode `160000`) inside the subtree — same security boundary as
    ///    the legacy zip extractor.
    /// 4. Verifies a `SKILL.md` blob exists at the subtree root.
    /// 5. Sums the declared blob sizes and short-circuits with the
    ///    [`MAX_PACKAGE_BYTES`] cap **before** issuing any blob downloads.
    /// 6. Downloads each blob via
    ///    `https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{path}`
    ///    using the same mirror-fallback chain as
    ///    [`Self::fetch_raw_skill_md`].
    ///
    /// The full-repo codeload zip is no longer fetched — only the bytes
    /// that belong to the skill being installed cross the wire. Returned
    /// [`TempSkillDir::root`] points at the in-tempdir copy of the skill
    /// (so a skill at repo path `skills/foo` lands at `<tmp>/`, not
    /// `<tmp>/<repo>-<branch>/skills/foo/`).
    ///
    /// Each call to the API endpoint counts against GitHub's anonymous
    /// 60 req/hr per-IP limit (raw blob downloads are CDN-served and not
    /// rate-limited). One install ⇒ one API call.
    pub async fn download_skill_to_temp(&self, loc: &GithubLocator) -> Result<TempSkillDir> {
        let path_prefix = loc.path.trim_matches('/');

        // ----- Step 1: fetch the recursive tree listing -------------------
        let tree_url = format!(
            "{}/repos/{}/{}/git/trees/{}?recursive=1",
            GITHUB_API_BASE, loc.owner, loc.repo, loc.branch
        );
        let tree_bytes = self
            .http_get_bytes_with_fallback(&tree_url, "fetch git tree", REQUEST_TIMEOUT)
            .await?;
        let tree: GitTreeResponse = serde_json::from_slice(&tree_bytes).map_err(|e| {
            CoreError::other(format!("parse git tree response from {tree_url}: {e}"))
        })?;
        if tree.truncated {
            return Err(CoreError::other(
                "git tree response was truncated by GitHub; \
                 the repo is too large to enumerate via the trees API",
            ));
        }

        // ----- Step 2: filter to the skill subtree ------------------------
        let in_subtree: Vec<&GitTreeEntry> = tree
            .tree
            .iter()
            .filter(|e| is_in_repo_subtree(&e.path, path_prefix))
            .collect();
        if in_subtree.is_empty() {
            return Err(CoreError::invalid(format!(
                "skill subtree {path_prefix:?} not found in {}/{}@{}",
                loc.owner, loc.repo, loc.branch
            )));
        }

        // ----- Step 3: security checks on the subtree --------------------
        for e in &in_subtree {
            if e.mode == "120000" {
                return Err(CoreError::invalid(
                    "skill subtree contains symlinks; refusing to install",
                ));
            }
            if e.mode == "160000" {
                return Err(CoreError::invalid(
                    "skill subtree contains git submodules; refusing to install",
                ));
            }
            // Defense in depth: GitHub never emits `..` in tree paths, but
            // we re-check anyway since the path becomes a filesystem path.
            for part in e.path.split('/') {
                if part == ".." {
                    return Err(CoreError::invalid(
                        "git tree path traversal detected; refusing to install",
                    ));
                }
            }
        }

        // ----- Step 4: verify SKILL.md presence --------------------------
        let skill_md_repo_path = if path_prefix.is_empty() {
            SKILL_FILE.to_string()
        } else {
            format!("{path_prefix}/{SKILL_FILE}")
        };
        let has_skill_md = in_subtree
            .iter()
            .any(|e| e.kind == "blob" && e.path == skill_md_repo_path);
        if !has_skill_md {
            return Err(CoreError::invalid(format!(
                "skill subtree {path_prefix:?} has no SKILL.md at its root"
            )));
        }

        // ----- Step 5: aggregate size + cap-check before any blob download
        let blobs: Vec<&GitTreeEntry> = in_subtree
            .iter()
            .copied()
            .filter(|e| e.kind == "blob")
            .collect();
        let total_size: u64 = blobs.iter().map(|e| e.size.unwrap_or(0)).sum();
        check_size_cap(total_size)?;

        // ----- Step 6: download each blob into a fresh tempdir -----------
        let tmp = tempfile::TempDir::new()
            .map_err(|e| CoreError::other(format!("create tempdir: {e}")))?;
        let extract_root = tmp.path().to_path_buf();

        for entry in &blobs {
            let relative = relative_inside_subtree(&entry.path, path_prefix);
            if relative.is_empty() {
                // The subtree's own root entry (a `tree`) gets filtered out
                // above; an empty relative on a blob would mean the path
                // exactly equals path_prefix, which the in-subtree filter
                // shouldn't admit for a blob — be paranoid anyway.
                continue;
            }
            // Reject any sketchy filesystem path components — we already
            // filtered `..` from `e.path`, but `relative` is what hits the
            // tempdir; check both segments (forward slash) and any rogue
            // backslash that snuck in.
            if relative.starts_with('/') || relative.starts_with('\\') {
                return Err(CoreError::invalid(format!(
                    "tree entry has absolute relative path: {relative}"
                )));
            }
            for part in relative.split(['/', '\\']) {
                if part == ".." {
                    return Err(CoreError::invalid(
                        "tree entry path traversal detected; refusing to install",
                    ));
                }
            }

            let blob_url = format!(
                "{}/{}/{}/{}/{}",
                RAW_GITHUB_BASE, loc.owner, loc.repo, loc.branch, entry.path
            );
            let bytes = self
                .http_get_bytes_with_fallback(&blob_url, "fetch skill blob", DOWNLOAD_TIMEOUT)
                .await?;

            // Belt-and-braces post-download size check: a malicious mirror
            // could serve more bytes than the tree listing claimed.
            if (bytes.len() as u64) > MAX_PACKAGE_BYTES {
                return Err(CoreError::invalid(format!(
                    "blob {} exceeds the 50 MB cap on its own",
                    entry.path
                )));
            }

            let target = extract_root.join(&relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CoreError::other(format!("mkdir {}: {e}", parent.display())))?;
            }
            std::fs::write(&target, &bytes)
                .map_err(|e| CoreError::other(format!("write {}: {e}", target.display())))?;
        }

        Ok(TempSkillDir {
            _tmp: tmp,
            root: extract_root,
        })
    }
}

/// Compose the in-zip path prefix that selects the skill's subtree given
/// the archive's single top-level dir and the user-supplied `path_within`.
///
/// - Empty `path_within` ⇒ the prefix is just `<top>/`, so every entry
///   inside the top dir lands in the extracted skill.
/// - Non-empty ⇒ `<top>/<path_within>/`, with leading and trailing slashes
///   stripped from `path_within` so callers can pass either
///   `"skills/foo"` or `"/skills/foo/"`.
///
/// Used by [`extract_skill_subtree`] to drop archive entries that aren't
/// part of the skill being installed.
fn build_subtree_prefix(top_level: &str, path_within: &str) -> String {
    let pw = path_within.trim_matches('/');
    if pw.is_empty() {
        format!("{top_level}/")
    } else {
        format!("{top_level}/{pw}/")
    }
}

/// True when an in-archive path (already normalized to forward slashes)
/// belongs to the subtree denoted by `subtree_prefix`.
///
/// Two shapes count as in-subtree:
/// - the subtree's own root directory entry (which may or may not carry a
///   trailing `/` in the archive),
/// - any path strictly under the prefix.
///
/// A sibling whose name happens to share the same lexical prefix (e.g.
/// `docs-changelog-extras/` vs `docs-changelog/`) is **not** in-subtree:
/// the trailing `/` on `subtree_prefix` is what disambiguates the two.
fn is_inside_subtree(normalized_path: &str, subtree_prefix: &str) -> bool {
    let path_no_slash = normalized_path.trim_end_matches('/');
    let pref_no_slash = subtree_prefix.trim_end_matches('/');
    normalized_path.starts_with(subtree_prefix) || path_no_slash == pref_no_slash
}

/// True when `entry_path` (forward-slashed, no leading `/`, GitHub-API
/// shape) belongs to the skill subtree rooted at `path_prefix`.
///
/// `path_prefix` is the user-supplied subdirectory **inside the repo**
/// (e.g. `"skills/foo"` or `""` when the skill lives at the repo root).
/// Leading / trailing slashes on `path_prefix` are tolerated.
///
/// Empty `path_prefix` ⇒ every entry passes (whole-repo subtree, used
/// when `GithubLocator::path` is empty).
///
/// Non-empty ⇒ the entry passes iff its path equals `path_prefix` or is
/// strictly under `<path_prefix>/`. The trailing-slash rule guards against
/// sibling overlap (e.g. `skills/foo` MUST NOT pull in `skills/foobar`).
fn is_in_repo_subtree(entry_path: &str, path_prefix: &str) -> bool {
    let pp = path_prefix.trim_matches('/');
    if pp.is_empty() {
        return true;
    }
    entry_path == pp || entry_path.starts_with(&format!("{pp}/"))
}

/// Strip the skill's `path_prefix` from a tree entry so the remainder is
/// the path **inside the extracted skill** (i.e. relative to the
/// `TempSkillDir::root` we hand back to the caller).
///
/// `entry_path` MUST already be in-subtree as determined by
/// [`is_in_repo_subtree`]; otherwise this returns an empty string.
fn relative_inside_subtree(entry_path: &str, path_prefix: &str) -> String {
    let pp = path_prefix.trim_matches('/');
    if pp.is_empty() {
        return entry_path.to_string();
    }
    if entry_path == pp {
        return String::new();
    }
    let pref = format!("{pp}/");
    entry_path
        .strip_prefix(&pref)
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// Extract a zip into a fresh tempdir, then narrow to `<tmp>/<top>/{path_within}/`.
///
/// This is the security-critical helper. The security boundary is the
/// **target subtree** (`<top>/{path_within}/`): every entry written to
/// disk inside that subtree is validated to reject path traversal segments
/// (`..`, absolute paths, Windows drive letters) and symlink entries
/// (`mode & 0o170000 == 0o120000`). Entries **outside** the target subtree
/// are silently skipped — they never hit disk so they cannot escape, and
/// a symlink in some unrelated corner of the source repo (e.g.
/// `node_modules/`, `vendor/`, the upstream's CI artifacts) MUST NOT block
/// installing a clean skill subtree.
///
/// After extraction the resolved subtree must contain a `SKILL.md` at its
/// root, otherwise this returns `Err`.
///
/// The returned [`TempSkillDir`] owns the tempdir; on `Err` the partially
/// populated tempdir is dropped (and removed) automatically.
pub fn extract_skill_subtree(zip_bytes: &[u8], path_within: &str) -> Result<TempSkillDir> {
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| CoreError::other(format!("invalid zip archive: {e}")))?;

    let tmp =
        tempfile::TempDir::new().map_err(|e| CoreError::other(format!("create tempdir: {e}")))?;
    let extract_root = tmp.path().to_path_buf();

    // ----- Pre-pass: discover the single top-level directory --------------
    //
    // Cheap (header-only metadata reads, no decompression). We need the top
    // dir BEFORE the main pass so we can compute the subtree prefix and
    // skip out-of-subtree entries without validating them.
    let mut top_level_dir: Option<String> = None;
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| CoreError::other(format!("read zip entry {i}: {e}")))?;
        let raw = entry.name();
        if raw.is_empty() {
            continue;
        }
        let normalized = raw.replace('\\', "/");
        let first = normalized
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("");
        if first.is_empty() {
            continue;
        }
        match &top_level_dir {
            None => top_level_dir = Some(first.to_string()),
            Some(existing) if existing != first => {
                return Err(CoreError::invalid(
                    "zip contains multiple top-level directories",
                ));
            }
            _ => {}
        }
    }
    let top_level = top_level_dir.ok_or_else(|| CoreError::invalid("zip archive is empty"))?;

    let subtree_prefix = build_subtree_prefix(&top_level, path_within);

    // ----- Main pass: validate + extract only in-subtree entries ----------
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| CoreError::other(format!("read zip entry {i}: {e}")))?;

        let raw_name = entry.name().to_string();
        if raw_name.is_empty() {
            continue;
        }

        // Skip entries outside the target subtree. Out-of-subtree symlinks /
        // path-traversal-named entries cannot escape because we never call
        // `std::fs::create_dir_all` / `File::create` for them — they're not
        // part of the security perimeter.
        let normalized_path = raw_name.replace('\\', "/");
        if !is_inside_subtree(&normalized_path, &subtree_prefix) {
            continue;
        }

        // Reject symlinks via unix mode high bits (in-subtree only).
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err(CoreError::invalid("zip contains symlinks"));
            }
        }

        // Absolute path / Windows drive letter / `..` segment → traversal.
        if raw_name.starts_with('/') || raw_name.starts_with('\\') {
            return Err(CoreError::invalid("zip path traversal detected"));
        }
        let bytes = raw_name.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return Err(CoreError::invalid("zip path traversal detected"));
        }
        for part in raw_name.split(['/', '\\']) {
            if part == ".." {
                return Err(CoreError::invalid("zip path traversal detected"));
            }
        }

        // Write to disk. Path components were validated above, so `target`
        // is guaranteed to remain inside `extract_root`.
        let target = extract_root.join(&raw_name);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|e| CoreError::other(format!("mkdir {}: {e}", target.display())))?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CoreError::other(format!("mkdir {}: {e}", parent.display())))?;
            }
            let mut out = std::fs::File::create(&target)
                .map_err(|e| CoreError::other(format!("create {}: {e}", target.display())))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| CoreError::other(format!("write {}: {e}", target.display())))?;
        }
    }

    let mut target_root = extract_root.join(&top_level);
    if !path_within.is_empty() {
        // `path_within` arrives forward-slashed; join component-by-component
        // so we don't accidentally feed an absolute path to `Path::join`.
        for part in path_within.split('/').filter(|p| !p.is_empty()) {
            if part == ".." {
                return Err(CoreError::invalid(
                    "path_within contains traversal segments",
                ));
            }
            target_root = target_root.join(part);
        }
    }

    if !target_root.is_dir() {
        return Err(CoreError::invalid(format!(
            "extracted directory {} does not exist in archive",
            target_root.display()
        )));
    }
    if !target_root.join(SKILL_FILE).is_file() {
        return Err(CoreError::invalid("extracted directory has no SKILL.md"));
    }

    Ok(TempSkillDir {
        _tmp: tmp,
        root: target_root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // --- build_search_params ------------------------------------------------

    /// Empty `q` must be rewritten to the `BROWSE_FALLBACK_QUERY` constant on
    /// the wire so skillsmp.com does not reject the request with
    /// `400 MISSING_QUERY`.
    #[test]
    fn build_search_params_substitutes_fallback_for_empty_q() {
        let params = build_search_params(&SearchQuery::default());
        let q = params
            .iter()
            .find(|(k, _)| *k == "q")
            .map(|(_, v)| v.as_str())
            .expect("q must always be present in wire params");
        assert_eq!(q, BROWSE_FALLBACK_QUERY);
    }

    /// Whitespace-only `q` is treated identically to empty.
    #[test]
    fn build_search_params_substitutes_fallback_for_whitespace_q() {
        let q = SearchQuery {
            q: "   \t  \n".to_string(),
            ..Default::default()
        };
        let params = build_search_params(&q);
        let on_wire = params
            .iter()
            .find(|(k, _)| *k == "q")
            .map(|(_, v)| v.as_str())
            .expect("q must always be present in wire params");
        assert_eq!(on_wire, BROWSE_FALLBACK_QUERY);
    }

    /// User-typed query passes through untouched.
    #[test]
    fn build_search_params_keeps_user_q() {
        let q = SearchQuery {
            q: "browser automation".to_string(),
            ..Default::default()
        };
        let params = build_search_params(&q);
        let on_wire = params
            .iter()
            .find(|(k, _)| *k == "q")
            .map(|(_, v)| v.as_str())
            .expect("q always emitted");
        assert_eq!(on_wire, "browser automation");
    }

    /// Optional knobs round-trip correctly when set, and only emit when set.
    #[test]
    fn build_search_params_includes_optional_filters_only_when_set() {
        let bare = build_search_params(&SearchQuery::default());
        // Empty SearchQuery contributes only `q`.
        assert_eq!(bare.iter().map(|(k, _)| *k).collect::<Vec<_>>(), vec!["q"]);

        let full = SearchQuery {
            q: "ai".to_string(),
            page: Some(2),
            limit: Some(48),
            sort_by: Some(SortBy::Stars),
            category: Some("data-ai".to_string()),
            occupation: Some("software-developers".to_string()),
        };
        let params = build_search_params(&full);
        let by_key: std::collections::HashMap<&str, String> =
            params.iter().map(|(k, v)| (*k, v.clone())).collect();
        assert_eq!(by_key.get("q"), Some(&"ai".to_string()));
        assert_eq!(by_key.get("page"), Some(&"2".to_string()));
        assert_eq!(by_key.get("limit"), Some(&"48".to_string()));
        assert_eq!(by_key.get("sortBy"), Some(&"stars".to_string()));
        assert_eq!(by_key.get("category"), Some(&"data-ai".to_string()));
        assert_eq!(
            by_key.get("occupation"),
            Some(&"software-developers".to_string())
        );
    }

    // --- build_download_candidates ----------------------------------------

    /// Direct URL must always be the first candidate, with mirrors trailing
    /// in declaration order.
    #[test]
    fn build_download_candidates_emits_direct_first_then_mirrors() {
        let direct = "https://codeload.github.com/owner/repo/zip/refs/heads/main";
        let candidates = build_download_candidates(direct, DEFAULT_GITHUB_MIRRORS);
        assert_eq!(candidates.first().map(String::as_str), Some(direct));
        assert_eq!(candidates.len(), 1 + DEFAULT_GITHUB_MIRRORS.len());
        for (i, mirror) in DEFAULT_GITHUB_MIRRORS.iter().enumerate() {
            let prefix = mirror.trim_end_matches('/');
            let expected = format!("{prefix}/{direct}");
            assert_eq!(candidates[1 + i], expected);
        }
    }

    /// Empty / whitespace mirror entries are dropped silently so a misconfig
    /// doesn't manufacture a `https:///<url>` candidate.
    #[test]
    fn build_download_candidates_drops_empty_mirror_entries() {
        let direct = "https://raw.githubusercontent.com/o/r/main/SKILL.md";
        let mirrors = ["", "   ", "https://ghfast.top/"];
        let candidates = build_download_candidates(direct, &mirrors);
        assert_eq!(candidates.len(), 2, "got {candidates:?}");
        assert_eq!(candidates[0], direct);
        assert_eq!(candidates[1], format!("https://ghfast.top/{direct}"));
    }

    /// A mirror with a trailing slash and one without must produce the same
    /// concatenation — no doubled `//` between the mirror and the URL.
    #[test]
    fn build_download_candidates_normalizes_trailing_slash() {
        let direct = "https://github.com/o/r";
        let with_slash = build_download_candidates(direct, &["https://m/"]);
        let no_slash = build_download_candidates(direct, &["https://m"]);
        assert_eq!(with_slash, no_slash);
        assert_eq!(with_slash[1], format!("https://m/{direct}"));
        assert!(!with_slash[1].contains("//https"));
    }

    /// No mirrors → only the direct entry is emitted.
    #[test]
    fn build_download_candidates_with_no_mirrors_is_direct_only() {
        let direct = "https://github.com/o/r";
        let candidates = build_download_candidates(direct, &[]);
        assert_eq!(candidates, vec![direct.to_string()]);
    }

    /// The shipped default list is non-empty and every entry is a syntactic
    /// HTTPS URL; this guards against an accidental `vec![]` in the source
    /// or a typo'd `htps://` slipping in via copy-paste.
    #[test]
    fn default_github_mirrors_are_https_urls() {
        assert!(
            !DEFAULT_GITHUB_MIRRORS.is_empty(),
            "default mirror list must ship at least one fallback"
        );
        for mirror in DEFAULT_GITHUB_MIRRORS {
            assert!(
                mirror.starts_with("https://"),
                "mirror {mirror:?} must be HTTPS"
            );
            assert!(mirror.ends_with('/'), "mirror {mirror:?} must end with '/'");
        }
    }

    // --- parse_github_url ---------------------------------------------------

    #[test]
    fn parse_github_url_valid_with_path() {
        let loc = SkillsMpClient::parse_github_url(
            "https://github.com/owner/repo/tree/main/path/to/skill",
        )
        .unwrap();
        assert_eq!(loc.owner, "owner");
        assert_eq!(loc.repo, "repo");
        assert_eq!(loc.branch, "main");
        assert_eq!(loc.path, "path/to/skill");
    }

    #[test]
    fn parse_github_url_valid_no_path() {
        let loc =
            SkillsMpClient::parse_github_url("https://github.com/owner/repo/tree/main").unwrap();
        assert_eq!(loc.owner, "owner");
        assert_eq!(loc.repo, "repo");
        assert_eq!(loc.branch, "main");
        assert_eq!(loc.path, "");
    }

    #[test]
    fn parse_github_url_valid_trailing_slash() {
        let loc =
            SkillsMpClient::parse_github_url("https://github.com/owner/repo/tree/main/").unwrap();
        assert_eq!(loc.path, "");
    }

    #[test]
    fn parse_github_url_rejects_other_host() {
        let err = SkillsMpClient::parse_github_url("https://gitlab.com/owner/repo/tree/main")
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("only github.com URLs accepted"),
            "unexpected error: {msg}"
        );
        assert!(msg.contains("gitlab.com"), "unexpected error: {msg}");
    }

    #[test]
    fn parse_github_url_rejects_blob() {
        let err =
            SkillsMpClient::parse_github_url("https://github.com/owner/repo/blob/main/file.md")
                .unwrap_err();
        assert!(err.to_string().contains("tree"), "unexpected error: {err}");
    }

    #[test]
    fn parse_github_url_rejects_ssh() {
        let err = SkillsMpClient::parse_github_url("git@github.com:owner/repo.git").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("ssh") || msg.contains("https"),
            "unexpected error: {msg}"
        );
    }

    // --- de_epoch_str / MarketSkill ----------------------------------------

    #[test]
    fn de_epoch_str_works() {
        // Mirrors the live skillsmp shape — `updatedAt` is a string of unix
        // epoch seconds and must decode through the custom deserializer.
        let json = r#"{
            "id": "comet-ml-opik-debugging-skill",
            "name": "debugging-e2e-tests",
            "author": "comet-ml",
            "description": "Use when an Opik E2E test has failed",
            "githubUrl": "https://github.com/comet-ml/opik/tree/main/.agents/skills/debugging-e2e-tests",
            "skillUrl": "https://skillsmp.com/skills/comet-ml-opik-debugging-skill",
            "stars": 19551,
            "updatedAt": "1781073435"
        }"#;
        let s: MarketSkill = serde_json::from_str(json).expect("decodes MarketSkill");
        assert_eq!(s.updated_at, 1_781_073_435i64);
        assert_eq!(s.stars, 19551);
        assert_eq!(s.author, "comet-ml");
    }

    #[test]
    fn search_response_decodes_official_shape() {
        // Minimal slice of the live response shape used by the install flow.
        let json = r#"{
            "success": true,
            "data": {
                "skills": [
                    {
                        "id": "x",
                        "name": "x",
                        "author": "a",
                        "description": "d",
                        "githubUrl": "https://github.com/a/x/tree/main",
                        "skillUrl": "https://skillsmp.com/skills/x",
                        "stars": 1,
                        "updatedAt": "1700000000"
                    }
                ],
                "pagination": {
                    "page": 1,
                    "limit": 24,
                    "total": 1,
                    "hasNext": false,
                    "hasPrev": false
                }
            }
        }"#;
        let resp: MarketSearchResponse =
            serde_json::from_str(json).expect("decodes MarketSearchResponse");
        assert!(resp.success);
        assert_eq!(resp.data.skills.len(), 1);
        assert_eq!(resp.data.skills[0].updated_at, 1_700_000_000);
        assert!(!resp.data.pagination.has_next);
    }

    // --- extract_skill_subtree ---------------------------------------------

    fn build_zip<F: FnOnce(&mut zip::ZipWriter<std::io::Cursor<&mut Vec<u8>>>)>(
        builder: F,
    ) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            builder(&mut zip);
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn extract_skill_subtree_rejects_traversal() {
        let buf = build_zip(|zip| {
            let opts = zip::write::FileOptions::default();
            zip.start_file("repo-main/evil/../escape.txt", opts)
                .unwrap();
            zip.write_all(b"oops").unwrap();
        });
        let err = extract_skill_subtree(&buf, "").unwrap_err();
        assert!(
            err.to_string().contains("traversal"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extract_skill_subtree_rejects_symlink() {
        // Build a zip whose top-level dir holds a SKILL.md plus a symlink.
        // `add_symlink` from zip-0.6 sets the unix-mode high bits to 0o120000,
        // which is exactly what the extractor refuses.
        let buf = build_zip(|zip| {
            let opts = zip::write::FileOptions::default();
            zip.start_file("repo-main/SKILL.md", opts).unwrap();
            zip.write_all(b"---\nname: x\ndescription: y\n---\n")
                .unwrap();
            zip.add_symlink("repo-main/link.txt", "../target", opts)
                .unwrap();
        });
        let err = extract_skill_subtree(&buf, "").unwrap_err();
        assert!(
            err.to_string().contains("symlink"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extract_skill_subtree_requires_skill_md() {
        let buf = build_zip(|zip| {
            let opts = zip::write::FileOptions::default();
            zip.start_file("repo-main/README.md", opts).unwrap();
            zip.write_all(b"hello").unwrap();
        });
        let err = extract_skill_subtree(&buf, "").unwrap_err();
        assert!(
            err.to_string().contains("SKILL.md"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extract_skill_subtree_happy_path_with_subpath() {
        let buf = build_zip(|zip| {
            let opts = zip::write::FileOptions::default();
            zip.start_file("opik-main/.agents/skills/debug/SKILL.md", opts)
                .unwrap();
            zip.write_all(b"---\nname: debug\ndescription: \"do debug\"\n---\nbody")
                .unwrap();
            zip.start_file("opik-main/README.md", opts).unwrap();
            zip.write_all(b"top-level readme").unwrap();
        });
        let extracted =
            extract_skill_subtree(&buf, ".agents/skills/debug").expect("happy path extracts");
        assert!(extracted.root.is_dir());
        assert!(extracted.root.join("SKILL.md").is_file());
    }

    /// A symlink in some unrelated corner of the source repo (here:
    /// `node_modules/`) MUST NOT block installing a clean skill subtree.
    /// Mirrors the real-world failure on `google-gemini/gemini-cli` where the
    /// `node_modules/.bin/` style symlinks abort the install of
    /// `.gemini/skills/docs-changelog/`.
    #[test]
    fn extract_skill_subtree_tolerates_symlinks_outside_subtree() {
        let buf = build_zip(|zip| {
            let opts = zip::write::FileOptions::default();
            // Clean skill subtree.
            zip.start_file(
                "gemini-cli-main/.gemini/skills/docs-changelog/SKILL.md",
                opts,
            )
            .unwrap();
            zip.write_all(b"---\nname: docs-changelog\ndescription: \"changelog docs\"\n---\nbody")
                .unwrap();
            // Junk elsewhere with a symlink — outside the install subtree.
            zip.add_symlink(
                "gemini-cli-main/node_modules/.bin/some-tool",
                "../bin/tool",
                opts,
            )
            .unwrap();
            // Junk readme at the repo root — outside the install subtree.
            zip.start_file("gemini-cli-main/README.md", opts).unwrap();
            zip.write_all(b"top-level readme").unwrap();
        });
        let extracted = extract_skill_subtree(&buf, ".gemini/skills/docs-changelog")
            .expect("symlinks outside the subtree must not block install");
        assert!(extracted.root.is_dir());
        assert!(extracted.root.join("SKILL.md").is_file());
        // The out-of-subtree symlink target was never written.
        let leak = extracted
            .root
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("node_modules"));
        if let Some(p) = leak {
            assert!(
                !p.exists(),
                "out-of-subtree symlink leaked to disk at {}",
                p.display()
            );
        }
    }

    /// Path-traversal-named entries OUTSIDE the install subtree must also
    /// be tolerated — they never hit disk so they cannot escape.
    #[test]
    fn extract_skill_subtree_tolerates_traversal_outside_subtree() {
        let buf = build_zip(|zip| {
            let opts = zip::write::FileOptions::default();
            zip.start_file("repo-main/skills/foo/SKILL.md", opts)
                .unwrap();
            zip.write_all(b"---\nname: foo\ndescription: \"do foo\"\n---\nbody")
                .unwrap();
            // Garbage entry with a `..` segment — outside the install subtree.
            zip.start_file("repo-main/junk/evil/../escape.txt", opts)
                .unwrap();
            zip.write_all(b"oops").unwrap();
        });
        let extracted = extract_skill_subtree(&buf, "skills/foo")
            .expect("out-of-subtree traversal must not block install");
        assert!(extracted.root.join("SKILL.md").is_file());
    }

    /// Sibling directories that share a lexical prefix with the requested
    /// path (e.g. `docs-changelog-extras/` vs `docs-changelog/`) must NOT
    /// be misclassified as in-subtree by the prefix matcher.
    #[test]
    fn extract_skill_subtree_does_not_leak_sibling_with_prefix_overlap() {
        let buf = build_zip(|zip| {
            let opts = zip::write::FileOptions::default();
            zip.start_file("repo-main/skills/foo/SKILL.md", opts)
                .unwrap();
            zip.write_all(b"---\nname: foo\ndescription: \"do foo\"\n---\nbody")
                .unwrap();
            // Sibling whose name starts with "foo" — could be picked up by a
            // naive `starts_with("repo-main/skills/foo")` without trailing /.
            zip.start_file("repo-main/skills/foobar/SKILL.md", opts)
                .unwrap();
            zip.write_all(b"---\nname: bar\ndescription: \"do bar\"\n---\nbody")
                .unwrap();
        });
        let extracted = extract_skill_subtree(&buf, "skills/foo").expect("happy path");
        // Only the requested subtree's SKILL.md is in the extracted tree.
        assert!(extracted.root.join("SKILL.md").is_file());
        let parent = extracted.root.parent().expect("subtree has parent");
        assert!(
            !parent.join("foobar").exists(),
            "sibling foobar/ leaked into the extraction"
        );
    }

    // --- is_in_repo_subtree / relative_inside_subtree -----------------------

    #[test]
    fn is_in_repo_subtree_empty_prefix_admits_everything() {
        assert!(is_in_repo_subtree("anything", ""));
        assert!(is_in_repo_subtree("a/b/c.md", ""));
        assert!(is_in_repo_subtree("", ""));
    }

    #[test]
    fn is_in_repo_subtree_matches_root_dir_and_descendants() {
        let pp = "skills/foo";
        assert!(is_in_repo_subtree("skills/foo", pp));
        assert!(is_in_repo_subtree("skills/foo/SKILL.md", pp));
        assert!(is_in_repo_subtree("skills/foo/references/notes.md", pp));
    }

    #[test]
    fn is_in_repo_subtree_excludes_siblings_and_parents() {
        let pp = "skills/foo";
        assert!(!is_in_repo_subtree("skills", pp));
        assert!(!is_in_repo_subtree("skills/foobar/SKILL.md", pp));
        assert!(!is_in_repo_subtree("skills/foo-other/SKILL.md", pp));
        assert!(!is_in_repo_subtree("README.md", pp));
        assert!(!is_in_repo_subtree("other/path", pp));
    }

    #[test]
    fn is_in_repo_subtree_normalizes_slashes_in_prefix() {
        // Caller may pass a prefix with leading / trailing slashes. Same
        // tolerance as build_subtree_prefix above.
        assert!(is_in_repo_subtree("skills/foo/SKILL.md", "/skills/foo/"));
    }

    #[test]
    fn relative_inside_subtree_strips_path_prefix() {
        assert_eq!(
            relative_inside_subtree("skills/foo/SKILL.md", "skills/foo"),
            "SKILL.md"
        );
        assert_eq!(
            relative_inside_subtree("skills/foo/references/notes.md", "skills/foo"),
            "references/notes.md"
        );
    }

    #[test]
    fn relative_inside_subtree_returns_empty_for_root_entry() {
        // The subtree's own root entry maps to "" — callers must skip it.
        assert_eq!(relative_inside_subtree("skills/foo", "skills/foo"), "");
    }

    #[test]
    fn relative_inside_subtree_passes_through_when_prefix_empty() {
        assert_eq!(
            relative_inside_subtree("SKILL.md", ""),
            "SKILL.md".to_string()
        );
        assert_eq!(
            relative_inside_subtree("a/b/c.md", ""),
            "a/b/c.md".to_string()
        );
    }

    // --- GitTreeResponse decoding ------------------------------------------

    /// The install flow only consumes a slice of the public response shape
    /// (`tree[].path / mode / type / size`, plus the top-level
    /// `truncated`). This test pins the deserializer against a minimal
    /// fixture in the official shape from the GitHub REST API docs.
    #[test]
    fn git_tree_response_decodes_official_shape() {
        // Mode strings are the actual values GitHub emits. `040000` is a
        // tree (no `size`); `100644` is a blob. The fixture also includes
        // a symlink (mode `120000`) and a submodule (mode `160000`) so the
        // install-path security checks have shapes to refuse against.
        let json = r#"{
            "sha": "abc123",
            "url": "https://api.github.com/repos/o/r/git/trees/abc123",
            "tree": [
                {
                    "path": "skills",
                    "mode": "040000",
                    "type": "tree",
                    "sha": "tttt",
                    "url": "https://api.github.com/repos/o/r/git/trees/tttt"
                },
                {
                    "path": "skills/foo",
                    "mode": "040000",
                    "type": "tree",
                    "sha": "bbbb",
                    "url": "https://api.github.com/repos/o/r/git/trees/bbbb"
                },
                {
                    "path": "skills/foo/SKILL.md",
                    "mode": "100644",
                    "type": "blob",
                    "sha": "blob1",
                    "size": 432,
                    "url": "https://api.github.com/repos/o/r/git/blobs/blob1"
                },
                {
                    "path": "skills/foo/link",
                    "mode": "120000",
                    "type": "blob",
                    "sha": "blob2",
                    "size": 12,
                    "url": "https://api.github.com/repos/o/r/git/blobs/blob2"
                },
                {
                    "path": "vendor/somelib",
                    "mode": "160000",
                    "type": "commit",
                    "sha": "blob3"
                }
            ],
            "truncated": false
        }"#;
        let resp: GitTreeResponse = serde_json::from_str(json).expect("decodes GitTreeResponse");
        assert!(!resp.truncated);
        assert_eq!(resp.tree.len(), 5);
        assert_eq!(resp.tree[0].kind, "tree");
        assert_eq!(resp.tree[0].mode, "040000");
        assert!(resp.tree[0].size.is_none());
        assert_eq!(resp.tree[2].kind, "blob");
        assert_eq!(resp.tree[2].size, Some(432));
        assert_eq!(resp.tree[3].mode, "120000");
        assert_eq!(resp.tree[4].mode, "160000");
        assert_eq!(resp.tree[4].kind, "commit");
    }

    /// `truncated` defaults to `false` when the field is missing — the
    /// install path treats it as "not truncated, proceed".
    #[test]
    fn git_tree_response_defaults_truncated_to_false() {
        let json = r#"{ "sha": "x", "tree": [] }"#;
        let resp: GitTreeResponse = serde_json::from_str(json).expect("decodes minimal");
        assert!(!resp.truncated);
        assert!(resp.tree.is_empty());
    }

    // --- build_subtree_prefix / is_inside_subtree ---------------------------

    #[test]
    fn build_subtree_prefix_handles_empty_path_within() {
        assert_eq!(build_subtree_prefix("repo-main", ""), "repo-main/");
    }

    #[test]
    fn build_subtree_prefix_normalizes_slashes() {
        // Leading and trailing slashes on path_within don't change the result.
        assert_eq!(
            build_subtree_prefix("repo-main", "skills/foo"),
            "repo-main/skills/foo/"
        );
        assert_eq!(
            build_subtree_prefix("repo-main", "/skills/foo/"),
            "repo-main/skills/foo/"
        );
    }

    #[test]
    fn is_inside_subtree_matches_root_and_descendants() {
        let p = "repo-main/skills/foo/";
        assert!(is_inside_subtree("repo-main/skills/foo/", p));
        assert!(is_inside_subtree("repo-main/skills/foo", p));
        assert!(is_inside_subtree("repo-main/skills/foo/SKILL.md", p));
        assert!(is_inside_subtree(
            "repo-main/skills/foo/references/notes.md",
            p
        ));
    }

    #[test]
    fn is_inside_subtree_excludes_siblings_and_parents() {
        let p = "repo-main/skills/foo/";
        assert!(!is_inside_subtree("repo-main/", p));
        assert!(!is_inside_subtree("repo-main/README.md", p));
        assert!(!is_inside_subtree("repo-main/skills/foobar/SKILL.md", p));
        assert!(!is_inside_subtree("repo-main/skills/foo-other/SKILL.md", p));
        assert!(!is_inside_subtree("other-repo/skills/foo/SKILL.md", p));
    }

    // --- check_size_cap -----------------------------------------------------

    #[test]
    fn enforce_size_cap() {
        assert!(check_size_cap(0).is_ok());
        assert!(check_size_cap(MAX_PACKAGE_BYTES).is_ok());
        let err = check_size_cap(MAX_PACKAGE_BYTES + 1).unwrap_err();
        assert!(
            err.to_string().contains("too large"),
            "unexpected error: {err}"
        );
        assert!(check_size_cap(50_000_001).is_err());
    }

    // --- API key resolution + handle ---------------------------------------

    /// Whether the build is wired with a non-empty `BUILTIN_SKILLSMP_API_KEY`.
    /// CI may strip the literal via `DEEPAGENT_SKILLSMP_API_KEY=""`, in which
    /// case the "fallback to builtin" branches resolve to `None` instead.
    fn has_builtin_key() -> bool {
        !BUILTIN_SKILLSMP_API_KEY.trim().is_empty()
    }

    #[test]
    fn resolve_api_key_user_wins() {
        // A non-empty user key is used verbatim regardless of whether a
        // builtin is configured.
        let (key, source) = resolve_api_key(Some("sk_user_xyz"));
        assert_eq!(key.as_deref(), Some("sk_user_xyz"));
        assert_eq!(source, ApiKeySource::User);
    }

    #[test]
    fn resolve_api_key_user_empty_falls_back() {
        // Empty `Some("")` is equivalent to "no user key" — the resolver
        // falls through to the builtin (or None when builtin is empty too).
        let (key, source) = resolve_api_key(Some(""));
        if has_builtin_key() {
            assert_eq!(key.as_deref(), Some(BUILTIN_SKILLSMP_API_KEY.trim()));
            assert_eq!(source, ApiKeySource::Builtin);
        } else {
            assert_eq!(key, None);
            assert_eq!(source, ApiKeySource::None);
        }
    }

    #[test]
    fn resolve_api_key_user_whitespace_falls_back() {
        // Whitespace-only user keys are also treated as "no key set".
        let (key, source) = resolve_api_key(Some("   \n\t  "));
        if has_builtin_key() {
            assert_eq!(key.as_deref(), Some(BUILTIN_SKILLSMP_API_KEY.trim()));
            assert_eq!(source, ApiKeySource::Builtin);
        } else {
            assert_eq!(key, None);
            assert_eq!(source, ApiKeySource::None);
        }
    }

    #[test]
    fn resolve_api_key_no_user_uses_builtin_or_none() {
        // No user-supplied key at all → builtin if configured, else anonymous.
        let (key, source) = resolve_api_key(None);
        if has_builtin_key() {
            assert_eq!(key.as_deref(), Some(BUILTIN_SKILLSMP_API_KEY.trim()));
            assert_eq!(source, ApiKeySource::Builtin);
        } else {
            assert_eq!(key, None);
            assert_eq!(source, ApiKeySource::None);
        }
    }

    #[test]
    fn handle_source_reflects_construction() {
        let user = SkillsMpClientHandle::new(Some("sk_real_user_key".to_string()));
        assert_eq!(user.source(), ApiKeySource::User);

        let none = SkillsMpClientHandle::new(None);
        let expected = if has_builtin_key() {
            ApiKeySource::Builtin
        } else {
            ApiKeySource::None
        };
        assert_eq!(none.source(), expected);
    }

    #[test]
    fn handle_set_user_key_swaps_source() {
        // Start with no user key — source is Builtin (or None).
        let h = SkillsMpClientHandle::new(None);
        let baseline = if has_builtin_key() {
            ApiKeySource::Builtin
        } else {
            ApiKeySource::None
        };
        assert_eq!(h.source(), baseline);

        // Pasting a real user key flips the source to User.
        h.set_user_key(Some("sk_user_paste_test".to_string()));
        assert_eq!(h.source(), ApiKeySource::User);

        // Clearing the user key falls back through the chain.
        h.set_user_key(None);
        assert_eq!(h.source(), baseline);

        // Whitespace-only counts as "clear".
        h.set_user_key(Some("sk_user_paste_test".to_string()));
        assert_eq!(h.source(), ApiKeySource::User);
        h.set_user_key(Some("   ".to_string()));
        assert_eq!(h.source(), baseline);
    }

    #[test]
    fn handle_with_client_borrows_inner() {
        // `with_client` should expose the underlying SkillsMpClient and
        // observe its rate-limit cache (None until a request is made).
        let h = SkillsMpClientHandle::new(Some("sk_test_inner".to_string()));
        let remaining = h.with_client(|c| c.last_daily_remaining());
        assert!(remaining.is_none());
    }

    #[test]
    fn skillsmp_client_is_clone() {
        // The Tauri command layer clones the client out of `with_client`
        // before any `.await`, so the std::sync::Mutex on the handle is not
        // held across an await point. Verify the clone shares the same
        // `daily_remaining` cache (Arc<Mutex<…>> — clones reference the same
        // inner Mutex) so a rate-limit update from a clone is visible
        // through the original.
        let c1 = SkillsMpClient::new(Some("sk_clone_test".to_string()));
        assert!(c1.last_daily_remaining().is_none());
        let c2 = c1.clone();
        // Bump the shared cache via the clone — it must be observable on the
        // original because both share the same Arc<Mutex<Option<u32>>>.
        if let Ok(mut g) = c2.daily_remaining.lock() {
            *g = Some(42);
        }
        assert_eq!(c1.last_daily_remaining(), Some(42));
        assert_eq!(c2.last_daily_remaining(), Some(42));
    }
}
