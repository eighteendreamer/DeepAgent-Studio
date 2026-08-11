import { useEffect, useMemo, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import type { MarketSkill, Skill, SkillActivation, SortBy } from "../types";
import {
  listSkills,
  reloadSkills,
  uninstallSkill,
  activateSkill,
  installSkillFromZip,
  isTauri,
  skillMarketSearch,
  getSkillInstallAiReviewEnabled,
} from "../api";
import { MarketSkillCard } from "./skills/MarketSkillCard";
import { SkillInstallDialog } from "./skills/SkillInstallDialog";
import { SkillsMarketProviderConfig } from "./skills/SkillsMarketProviderConfig";

// Map a skill id (or name) to an icon + accent color, falling back to a cube.
function visualFor(skill: Skill): { icon: IconProp; bg: string } {
  const map: Record<string, { icon: IconProp; bg: string }> = {
    "agent-browser": { icon: ["fab", "chrome"], bg: "bg-gray-100 text-gray-600" },
    "code-review-skill": { icon: ["fas", "code"], bg: "bg-purple-100 text-purple-600" },
    "mcp-builder": { icon: ["fas", "server"], bg: "bg-indigo-100 text-indigo-600" },
    "planning-with-files": { icon: ["fas", "pen"], bg: "bg-blue-100 text-blue-600" },
    "rust-backend-review": { icon: ["fas", "code-branch"], bg: "bg-orange-100 text-orange-600" },
    superpowers: { icon: ["fas", "bullseye"], bg: "bg-pink-100 text-pink-600" },
    "ui-ux-pro-max-skill": { icon: ["fas", "layer-group"], bg: "bg-green-100 text-green-600" },
    "webapp-testing": { icon: ["fas", "terminal"], bg: "bg-yellow-100 text-yellow-700" },
  };
  return map[skill.id] ?? { icon: ["fas", "cube"], bg: "bg-gray-100 text-gray-600" };
}

function useOriginLabel() {
  const { t } = useTranslation();
  return (origin: string): string => {
    switch (origin) {
      case "installed":
        return t("skillsView.origin.installed");
      case "workspace":
        return t("skillsView.origin.workspace");
      case "user":
        return t("skillsView.origin.user");
      case "built_in":
        return t("skillsView.origin.builtIn");
      default:
        return origin;
    }
  };
}

type MarketTab = "market" | "installed";

const MARKET_PAGE_LIMIT = 24;
const MARKET_SORT_BY: SortBy = "stars"; // hardcoded for v1; settings UI is task 21

export function SkillsView() {
  const { t } = useTranslation();
  const originLabel = useOriginLabel();

  // ---------------- shared state ----------------
  const [marketTab, setMarketTab] = useState<MarketTab>("installed"); // R8.1: default = installed
  const [search, setSearch] = useState("");

  // ---------------- installed-tab state ----------------
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<Skill | null>(null);
  const [activation, setActivation] = useState<SkillActivation | null>(null);

  // ---------------- market-tab state ----------------
  const [marketSkills, setMarketSkills] = useState<MarketSkill[]>([]);
  const [marketTotal, setMarketTotal] = useState(0);
  const [marketPage, setMarketPage] = useState(1);
  const [marketHasNext, setMarketHasNext] = useState(false);
  const [marketLoading, setMarketLoading] = useState(false);
  const [marketError, setMarketError] = useState<string | null>(null);
  // Track whether the market tab has run its first search yet, so we trigger
  // the empty-q stars-sorted load lazily on first switch (R8.2).
  const marketLoadedOnce = useRef(false);

  // ---------------- install-dialog state ----------------
  // `installSource` drives the controlled SkillInstallDialog: non-null = open
  // and scanning, null = hidden. `aiReviewEnabled` mirrors the persisted
  // AppSettings value (R10.3 / R10.4) so the dialog can skip the review block
  // entirely when the user opted out.
  const [installSource, setInstallSource] = useState<{
    githubUrl: string;
    skill: MarketSkill;
  } | null>(null);
  const [aiReviewEnabled, setAiReviewEnabled] = useState(true);

  // Provider Config popover toggle (R9.2 / R9.4-R9.6).
  const [providerConfigOpen, setProviderConfigOpen] = useState(false);

  useEffect(() => {
    void getSkillInstallAiReviewEnabled().then(setAiReviewEnabled);
  }, []);

  async function refresh(rescan = false) {
    setLoading(true);
    try {
      const list = rescan ? await reloadSkills() : await listSkills();
      setSkills(list);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh(false);
  }, []);

  // -- Market: lazy first-load on tab switch; debounced re-search on query --
  useEffect(() => {
    if (marketTab !== "market") return;
    if (!marketLoadedOnce.current) {
      marketLoadedOnce.current = true;
      void runMarketSearch(search, 1, /* append */ false);
      return;
    }
    // Debounce 300ms when the search box changes while we're on the market tab.
    const handle = window.setTimeout(() => {
      void runMarketSearch(search, 1, /* append */ false);
    }, 300);
    return () => window.clearTimeout(handle);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [marketTab, search]);

  async function runMarketSearch(q: string, page: number, append: boolean) {
    setMarketLoading(true);
    setMarketError(null);
    try {
      const data = await skillMarketSearch({
        q: q.trim() || undefined,
        page,
        limit: MARKET_PAGE_LIMIT,
        sortBy: MARKET_SORT_BY,
      });
      setMarketSkills((prev) => (append ? [...prev, ...data.skills] : data.skills));
      setMarketTotal(data.pagination.total);
      setMarketHasNext(data.pagination.hasNext);
      setMarketPage(data.pagination.page);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setMarketError(msg);
      if (!append) {
        setMarketSkills([]);
        setMarketTotal(0);
        setMarketHasNext(false);
      }
    } finally {
      setMarketLoading(false);
    }
  }

  async function handleLoadMore() {
    if (!marketHasNext || marketLoading) return;
    await runMarketSearch(search, marketPage + 1, /* append */ true);
  }

  async function handleInstallZip() {
    if (!isTauri()) {
      alert("ZIP install requires the desktop app");
      return;
    }
    try {
      const mod = await import("@tauri-apps/plugin-dialog");
      const sel = await mod.open({
        multiple: false,
        filters: [{ name: "Zip Archives", extensions: ["zip"] }],
        title: "Select Skill ZIP",
      });
      if (typeof sel === "string") {
        setLoading(true);
        await installSkillFromZip(sel);
        await refresh(true);
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      alert("Failed to install ZIP: " + msg);
      setLoading(false);
    }
  }

  function handleProviderConfig() {
    setProviderConfigOpen((v) => !v);
  }

  function handleMarketInstall(skill: MarketSkill) {
    // Open the controlled SkillInstallDialog. The dialog kicks off
    // skillMarketScan internally and surfaces the static-scan + AI review.
    setInstallSource({ githubUrl: skill.githubUrl, skill });
  }

  function handleInstallDialogClose(installed: Skill | null) {
    setInstallSource(null);
    if (installed) {
      // Re-list so the freshly installed skill appears in the Installed tab
      // and the market card flips to "✓ Installed".
      void refresh(true);
    }
  }

  // -- Installed tab: client-side filter --
  const filteredInstalled = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return skills;
    return skills.filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        s.description.toLowerCase().includes(q) ||
        s.triggers.some((tp) => tp.toLowerCase().includes(q))
    );
  }, [skills, search]);

  // Lookup of installed skill ids (lowercased) for marking market cards as
  // already-installed (R8.4).
  //
  // SkillsMP returns synthesized ids that look like the GitHub slug
  // (`aidotnet-opencowork-resources-skills-md-to-office-skill-md`), while a
  // locally registered Skill carries a clean id (`md-to-office`). For v1 we
  // fall back to a name-based match: a market skill is considered installed
  // when its `name` equals one of the installed skill ids, lowercased.
  // TODO: revisit when SkillsService.install_from_temp emits a stable cross-
  // origin id mapping.
  const installedNamesSet = useMemo(
    () =>
      new Set<string>(
        skills.flatMap((s) => [s.id.toLowerCase(), s.name.toLowerCase()])
      ),
    [skills]
  );

  function isMarketSkillInstalled(s: MarketSkill): boolean {
    return (
      installedNamesSet.has(s.id.toLowerCase()) ||
      installedNamesSet.has(s.name.toLowerCase())
    );
  }

  async function onSelect(skill: Skill) {
    setSelected(skill);
    setActivation(await activateSkill(skill.id));
  }

  async function onUninstall(skill: Skill) {
    await uninstallSkill(skill.id);
    if (selected?.id === skill.id) {
      setSelected(null);
      setActivation(null);
    }
    await refresh(false);
  }

  // ---------------- render ----------------
  const toolbarIconBtn =
    "flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-ui-tint text-text-secondary transition-colors duration-150 ease-out hover:bg-ui-tint-strong hover:text-text-base disabled:cursor-not-allowed disabled:opacity-40";

  return (
    <div className="w-full h-full flex bg-white overflow-hidden">
      <div className="flex-1 flex flex-col overflow-y-auto px-12 py-10">
        {/* Page header */}
        <div className="mb-6 w-full max-w-5xl mx-auto">
          <h1 className="text-3xl font-semibold text-text-base mb-2">{t("skillsView.title")}</h1>
          <p className="text-sm text-text-secondary">
            {t("skillsView.subtitle1")}
            <code className="text-[12px] bg-gray-100 px-1.5 py-0.5 rounded">.deepagent/skills</code>
            {t("skillsView.subtitle2")}
          </p>
        </div>

        {/* Top toolbar: tab pills + search + tab-specific actions */}
        <div className="flex items-center justify-between gap-3 w-full max-w-5xl mx-auto mb-6 flex-wrap">
          {/* Tab pills */}
          <div className="inline-flex items-center bg-gray-100 rounded-full p-1">
            <button
              onClick={() => setMarketTab("market")}
              className={`px-4 py-1.5 text-sm rounded-full transition-colors ${
                marketTab === "market"
                  ? "bg-white text-text-base shadow-sm"
                  : "text-text-secondary hover:text-text-base"
              }`}
            >
              {t("skillsView.tab_market")}
            </button>
            <button
              onClick={() => {
                setMarketTab("installed");
                setProviderConfigOpen(false);
              }}
              className={`px-4 py-1.5 text-sm rounded-full transition-colors ${
                marketTab === "installed"
                  ? "bg-white text-text-base shadow-sm"
                  : "text-text-secondary hover:text-text-base"
              }`}
            >
              {t("skillsView.tab_installed")}
            </button>
          </div>

          <div className="ml-auto flex items-center gap-2">
            <div className="flex h-8 w-64 min-w-[12rem] items-center rounded-lg bg-ui-tint transition-colors duration-150 ease-out focus-within:bg-ui-tint-strong">
              <FontAwesomeIcon
                icon={["fas", "magnifying-glass"]}
                className="ml-3 shrink-0 text-[12px] text-text-secondary"
              />
              <input
                type="search"
                placeholder={t("skillsView.market_search_placeholder")}
                className="h-full min-w-0 flex-1 border-0 bg-transparent pl-2 pr-3 text-[12px] text-text-base outline-none placeholder:text-text-secondary"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
            </div>

            {marketTab === "installed" && (
              <button
                type="button"
                onClick={() => refresh(true)}
                title={t("skillsView.refresh")}
                className={toolbarIconBtn}
              >
                <FontAwesomeIcon
                  icon={["fas", "rotate-right"]}
                  className={`text-[12px] ${loading ? "animate-spin" : ""}`}
                />
              </button>
            )}

            {marketTab === "installed" && (
              <button
                type="button"
                onClick={handleInstallZip}
                className={toolbarIconBtn}
                title="Upload ZIP Skill"
              >
                <FontAwesomeIcon icon={["fas", "file-zipper"]} className="text-[12px]" />
              </button>
            )}

            {marketTab === "market" && (
              <div className="relative">
                <button
                  type="button"
                  onClick={handleProviderConfig}
                  className={`${toolbarIconBtn} ${providerConfigOpen ? "bg-ui-tint-strong text-text-base" : ""}`}
                  title={t("skillsView.market_provider_config")}
                  aria-haspopup="dialog"
                  aria-expanded={providerConfigOpen}
                >
                  <FontAwesomeIcon icon={["fas", "gear"]} className="text-[12px]" />
                </button>
                <SkillsMarketProviderConfig
                  open={providerConfigOpen}
                  onClose={() => setProviderConfigOpen(false)}
                />
              </div>
            )}
          </div>
        </div>

        {/* Body */}
        {marketTab === "installed" ? (
          <InstalledBody
            loading={loading}
            skills={skills}
            filtered={filteredInstalled}
            selected={selected}
            originLabel={originLabel}
            onSelect={onSelect}
            onUninstall={onUninstall}
          />
        ) : (
          <MarketBody
            marketSkills={marketSkills}
            marketTotal={marketTotal}
            marketHasNext={marketHasNext}
            marketLoading={marketLoading}
            marketError={marketError}
            onLoadMore={handleLoadMore}
            onInstall={handleMarketInstall}
            isInstalled={isMarketSkillInstalled}
          />
        )}
      </div>

      {/* Right-side detail pane (Installed tab only) */}
      {marketTab === "installed" && selected && (
        <div className="w-96 border-l border-border-theme flex flex-col overflow-hidden bg-gray-50/50">
          <div className="px-6 py-5 border-b border-border-theme flex items-start justify-between">
            <div>
              <div className="text-lg font-semibold text-text-base">{selected.name}</div>
              <div className="text-xs text-text-secondary mt-0.5">
                {selected.id} · {originLabel(selected.origin)}
                {selected.version ? ` · v${selected.version}` : ""}
              </div>
            </div>
            <div className="flex items-center gap-2">
              {/* R8.7 / R8.8: hide uninstall for built-in skills */}
              {selected.origin !== "built_in" && (
                <button
                  onClick={() => onUninstall(selected)}
                  title={t("skillsView.uninstall")}
                  className="w-7 h-7 rounded-full border border-border-theme flex items-center justify-center text-text-secondary hover:bg-white hover:text-red-500 transition-all bg-white"
                >
                  <FontAwesomeIcon icon={["fas", "xmark"]} className="text-xs" />
                </button>
              )}
              <button
                onClick={() => {
                  setSelected(null);
                  setActivation(null);
                }}
                className="text-text-secondary hover:text-text-base"
                title="Close"
              >
                <FontAwesomeIcon icon={["fas", "xmark"]} />
              </button>
            </div>
          </div>
          <div className="flex-1 overflow-y-auto px-6 py-4">
            <div className="text-xs font-medium text-text-secondary uppercase tracking-wide mb-2">
              {t("skillsView.triggerPhrases", { count: selected.triggers.length })}
            </div>
            <div className="flex flex-wrap gap-1.5 mb-5">
              {selected.triggers.map((tp) => (
                <span
                  key={tp}
                  className="text-[11px] bg-white border border-border-theme rounded-full px-2 py-0.5 text-text-secondary"
                >
                  {tp}
                </span>
              ))}
            </div>
            <div className="text-xs font-medium text-text-secondary uppercase tracking-wide mb-2">
              {t("skillsView.body")}
            </div>
            <pre className="text-[12px] text-text-base whitespace-pre-wrap font-mono leading-relaxed bg-white border border-border-theme rounded-lg p-3">
              {activation?.body ?? t("skillsView.noBody")}
            </pre>
          </div>
        </div>
      )}

      {/* Install dialog (controlled by `installSource`) — visible across tabs
          so a transient tab switch doesn't drop a pending scan / review. */}
      <SkillInstallDialog
        source={installSource}
        aiReviewEnabled={aiReviewEnabled}
        onClose={handleInstallDialogClose}
      />
    </div>
  );
}

// =============================================================================
// Installed tab body
// =============================================================================

interface InstalledBodyProps {
  loading: boolean;
  skills: Skill[];
  filtered: Skill[];
  selected: Skill | null;
  originLabel: (o: string) => string;
  onSelect: (s: Skill) => void;
  onUninstall: (s: Skill) => void;
}

function InstalledBody({
  loading,
  skills,
  filtered,
  selected,
  originLabel,
  onSelect,
  onUninstall,
}: InstalledBodyProps) {
  const { t } = useTranslation();
  return (
    <div className="w-full max-w-5xl mx-auto">
      <h2 className="text-base font-medium text-text-base mb-1">
        {t("skillsView.discoveredCount", { count: filtered.length })}
      </h2>
      <p className="text-xs text-text-secondary mb-4">{t("skillsView.clickToView")}</p>

      {loading && skills.length === 0 ? (
        <div className="text-sm text-text-secondary py-10 text-center">
          {t("skillsView.loading")}
        </div>
      ) : filtered.length === 0 ? (
        <div className="text-sm text-text-secondary py-10 text-center">
          {t("skillsView.noSkills")}
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-x-6 gap-y-2">
          {filtered.map((skill) => {
            const v = visualFor(skill);
            const active = selected?.id === skill.id;
            const builtIn = skill.origin === "built_in";
            return (
              <div
                key={skill.id}
                onClick={() => onSelect(skill)}
                className={`flex items-center p-3 rounded-xl cursor-pointer transition-colors group ${
                  active ? "bg-gray-100" : "hover:bg-black/5"
                }`}
              >
                <div
                  className={`w-10 h-10 rounded-lg flex items-center justify-center flex-shrink-0 mr-4 ${v.bg}`}
                >
                  <FontAwesomeIcon icon={v.icon} className="text-lg" />
                </div>
                <div className="flex-1 min-w-0 pr-3">
                  <div className="flex items-center gap-2">
                    <span className="text-[14px] font-medium text-text-base truncate">
                      {skill.name}
                    </span>
                    <span className="text-[10px] text-text-secondary border border-border-theme rounded-full px-1.5 py-0.5 flex-shrink-0">
                      {originLabel(skill.origin)}
                    </span>
                  </div>
                  <div className="text-[12px] text-text-secondary truncate mt-0.5">
                    {skill.description}
                  </div>
                </div>
                {/* R8.7 / R8.8: hide uninstall (✕) for built-in skills */}
                {!builtIn && (
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      onUninstall(skill);
                    }}
                    title={t("skillsView.uninstall")}
                    className="w-7 h-7 rounded-full border border-border-theme flex items-center justify-center text-text-secondary hover:bg-white hover:text-red-500 transition-all bg-gray-50 opacity-0 group-hover:opacity-100"
                  >
                    <FontAwesomeIcon icon={["fas", "xmark"]} className="text-xs" />
                  </button>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

// =============================================================================
// Market tab body
// =============================================================================

interface MarketBodyProps {
  marketSkills: MarketSkill[];
  marketTotal: number;
  marketHasNext: boolean;
  marketLoading: boolean;
  marketError: string | null;
  onLoadMore: () => void;
  onInstall: (s: MarketSkill) => void;
  isInstalled: (s: MarketSkill) => boolean;
}

function MarketBody({
  marketSkills,
  marketTotal,
  marketHasNext,
  marketLoading,
  marketError,
  onLoadMore,
  onInstall,
  isInstalled,
}: MarketBodyProps) {
  const { t } = useTranslation();
  return (
    <div className="w-full max-w-6xl mx-auto">
      {marketError && (
        <div className="text-sm text-red-600 bg-red-50 border border-red-200 rounded-lg px-4 py-3 mb-4">
          {marketError}
        </div>
      )}

      {marketLoading && marketSkills.length === 0 ? (
        <div className="text-sm text-text-secondary py-10 text-center">
          {t("skillsView.market_loading")}
        </div>
      ) : marketSkills.length === 0 ? (
        <div className="text-sm text-text-secondary py-10 text-center">
          {t("skillsView.market_no_results")}
        </div>
      ) : (
        <>
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
            {marketSkills.map((skill) => (
              <MarketSkillCard
                key={skill.id}
                skill={skill}
                installed={isInstalled(skill)}
                onInstall={onInstall}
              />
            ))}
          </div>

          {marketHasNext && (
            <div className="flex items-center justify-center mt-6">
              <button
                onClick={onLoadMore}
                disabled={marketLoading}
                className="px-4 py-2 text-sm rounded-full border border-border-theme bg-white text-text-base hover:bg-black/5 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
              >
                {marketLoading
                  ? t("skillsView.market_loading")
                  : `${t("skillsView.market_load_more")} (${marketSkills.length}/${marketTotal})`}
              </button>
            </div>
          )}
        </>
      )}
    </div>
  );
}
