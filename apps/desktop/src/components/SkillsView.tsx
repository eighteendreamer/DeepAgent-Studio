import { useEffect, useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import type { Skill, SkillActivation } from "../types";
import {
  listSkills,
  reloadSkills,
  uninstallSkill,
  activateSkill,
  installSkillFromZip,
  isTauri,
} from "../api";

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

const RECOMMENDED_SKILLS = [
  { id: "aspnet-core", name: ".NET Aspnet Core", description: "[Windows only] Build and review ASP.NET...", iconText: ".NET", bg: "bg-[#512bd4] text-white" },
  { id: "chatgpt-apps", name: "Chatgpt Apps", description: "Build and scaffold ChatGPT apps", icon: ["fas", "cube"] as any, bg: "bg-gradient-to-br from-yellow-400 to-orange-500 text-white" },
  { id: "cli-creator", name: "CLI Creator", description: "Build CLIs for Codex", icon: ["fas", "cube"] as any, bg: "bg-gradient-to-br from-purple-400 to-blue-500 text-white" },
  { id: "cloudflare-deploy", name: "Cloudflare Deploy", description: "Deploy Workers, Pages, and platform service...", icon: ["fas", "cloud"] as any, bg: "bg-[#f38020] text-white" },
  { id: "define-goal", name: "Define Goal", description: "Shape clear measurable goals", icon: ["fas", "cube"] as any, bg: "bg-gradient-to-br from-pink-400 to-purple-500 text-white" },
  { id: "figma", name: "Figma", description: "Use Figma MCP for design-to-code work", icon: ["fab", "figma"] as any, bg: "bg-black text-white" },
  { id: "figma-code-connect", name: "Figma Code Connect Components", description: "Map Figma components to code with Code...", icon: ["fab", "figma"] as any, bg: "bg-black text-white" },
  { id: "figma-design-system", name: "Figma Create Design System Rules", description: "Generate design system rules for your...", icon: ["fab", "figma"] as any, bg: "bg-black text-white" },
  { id: "figma-implement-design", name: "Figma Implement Design", description: "Turn Figma designs into production-ready...", icon: ["fab", "figma"] as any, bg: "bg-black text-white" },
  { id: "gh-address-comments", name: "GH Address Comments", description: "Address comments in a GitHub PR review", icon: ["fab", "github"] as any, bg: "bg-white text-gray-800 border border-border-theme shadow-sm" },
  { id: "gh-fix-ci", name: "GH Fix CI", description: "Debug failing GitHub Actions CI", icon: ["fab", "github"] as any, bg: "bg-white text-gray-800 border border-border-theme shadow-sm" },
  { id: "jupyter-notebook", name: "Jupyter Notebook", description: "Create Jupyter notebooks for experiments...", icon: ["fas", "circle-notch"] as any, bg: "bg-white text-orange-500 border border-border-theme shadow-sm" },
  { id: "linear", name: "Linear", description: "Manage Linear issues in Codex", icon: ["fas", "chart-gantt"] as any, bg: "bg-black text-white" },
  { id: "migrate-to-codex", name: "Migrate to Codex", description: "Migrate supported instruction files, skills,...", icon: ["fas", "clock"] as any, bg: "bg-gray-100 text-gray-500" },
];

export function SkillsView() {
  const { t } = useTranslation();
  const originLabel = useOriginLabel();
  const [search, setSearch] = useState("");
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<Skill | null>(null);
  const [activation, setActivation] = useState<SkillActivation | null>(null);

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

  async function handleInstallZip() {
    if (!isTauri()) {
      alert("ZIP install requires the desktop app");
      return;
    }
    try {
      const mod = await import("@tauri-apps/plugin-dialog");
      const selected = await mod.open({
        multiple: false,
        filters: [{ name: "Zip Archives", extensions: ["zip"] }],
        title: "Select Skill ZIP",
      });
      if (typeof selected === "string") {
        setLoading(true);
        await installSkillFromZip(selected);
        await refresh(true);
      }
    } catch (e: any) {
      alert("Failed to install ZIP: " + e.message);
      setLoading(false);
    }
  }

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return skills;
    return skills.filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        s.description.toLowerCase().includes(q) ||
        s.triggers.some((t) => t.includes(q))
    );
  }, [skills, search]);

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

  return (
    <div className="w-full h-full flex bg-white overflow-hidden">
      {/* Left: skill catalog */}
      <div className="flex-1 flex flex-col overflow-y-auto px-12 py-10">
        {/* Header */}
        <div className="flex items-start justify-between mb-8 w-full max-w-4xl mx-auto">
          <div>
            <h1 className="text-3xl font-semibold text-text-base mb-2">{t("skillsView.title")}</h1>
            <p className="text-sm text-text-secondary">
              {t("skillsView.subtitle1")}<code className="text-[12px] bg-gray-100 px-1.5 py-0.5 rounded">.deepagent/skills</code>{t("skillsView.subtitle2")}
            </p>
          </div>
          <div className="flex items-center space-x-3">
            <button
              onClick={() => refresh(true)}
              className="flex items-center text-text-secondary hover:text-text-base cursor-pointer text-sm transition-colors"
            >
              <FontAwesomeIcon icon={["fas", "rotate-right"]} className={`mr-1.5 ${loading ? "animate-spin" : ""}`} />
              {t("skillsView.refresh")}
            </button>
            <div className="flex items-center bg-gray-50 border border-border-theme rounded-full px-3 py-1.5 w-64 focus-within:border-gray-300 focus-within:bg-white transition-all">
              <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className="text-text-secondary text-sm mr-2" />
              <input
                type="text"
                placeholder={t("skillsView.searchPlaceholder")}
                className="bg-transparent outline-none w-full text-sm text-text-base"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
            </div>
            <button
              onClick={handleInstallZip}
              className="flex items-center justify-center w-8 h-8 rounded border border-border-theme text-text-secondary hover:text-text-base hover:bg-gray-50 transition-colors"
              title="Upload ZIP Skill"
            >
              <FontAwesomeIcon icon={["fas", "upload"]} className="text-sm" />
            </button>
          </div>
        </div>

        <div className="w-full max-w-4xl mx-auto">
          <h2 className="text-base font-medium text-text-base mb-1">
            {t("skillsView.discoveredCount", { count: filtered.length })}
          </h2>
          <p className="text-xs text-text-secondary mb-4">
            {t("skillsView.clickToView")}
          </p>

          {loading && skills.length === 0 ? (
            <div className="text-sm text-text-secondary py-10 text-center">{t("skillsView.loading")}</div>
          ) : filtered.length === 0 ? (
            <div className="text-sm text-text-secondary py-10 text-center">
              {t("skillsView.noSkills")}
            </div>
          ) : (
            <div className="grid grid-cols-2 gap-x-6 gap-y-2">
              {filtered.map((skill) => {
                const v = visualFor(skill);
                const active = selected?.id === skill.id;
                return (
                  <div
                    key={skill.id}
                    onClick={() => onSelect(skill)}
                    className={`flex items-center p-3 rounded-xl cursor-pointer transition-colors group ${
                      active ? "bg-gray-100" : "hover:bg-gray-50"
                    }`}
                  >
                    <div className={`w-10 h-10 rounded-lg flex items-center justify-center flex-shrink-0 mr-4 ${v.bg}`}>
                      <FontAwesomeIcon icon={v.icon} className="text-lg" />
                    </div>
                    <div className="flex-1 min-w-0 pr-3">
                      <div className="flex items-center gap-2">
                        <span className="text-[14px] font-medium text-text-base truncate">{skill.name}</span>
                        <span className="text-[10px] text-text-secondary border border-border-theme rounded-full px-1.5 py-0.5 flex-shrink-0">
                          {originLabel(skill.origin)}
                        </span>
                      </div>
                      <div className="text-[12px] text-text-secondary truncate mt-0.5">{skill.description}</div>
                    </div>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        onUninstall(skill);
                      }}
                      title={t("skillsView.uninstall")}
                      className="w-7 h-7 rounded-full border border-border-theme flex items-center justify-center text-text-secondary hover:bg-white hover:text-red-500 transition-all bg-gray-50 opacity-0 group-hover:opacity-100"
                    >
                      <FontAwesomeIcon icon={["fas", "minus"]} className="text-xs" />
                    </button>
                  </div>
                );
              })}
            </div>
          )}

          {/* Recommended Skills */}
          <div className="mt-10 pt-6">
            <h2 className="text-base font-medium text-text-base mb-4">{t("skillsView.recommended")}</h2>
            <div className="grid grid-cols-2 gap-x-6 gap-y-2">
              {RECOMMENDED_SKILLS.map((item) => (
                <div
                  key={item.id}
                  className="flex items-center p-3 rounded-xl cursor-pointer transition-colors group hover:bg-gray-100"
                >
                  <div className={`w-10 h-10 rounded-lg flex items-center justify-center flex-shrink-0 mr-4 ${item.bg}`}>
                    {item.iconText ? (
                      <span className="text-[10px] font-bold tracking-wider">{item.iconText}</span>
                    ) : item.icon ? (
                      <FontAwesomeIcon icon={item.icon} className="text-lg" />
                    ) : null}
                  </div>
                  <div className="flex-1 min-w-0 pr-3">
                    <div className="text-[14px] font-medium text-text-base truncate">{item.name}</div>
                    <div className="text-[12px] text-text-secondary truncate mt-0.5">{item.description}</div>
                  </div>
                  <button
                    className="w-7 h-7 rounded-full border border-border-theme flex items-center justify-center text-text-secondary bg-gray-50 hover:bg-white hover:text-text-base transition-all"
                    title={t("skillsView.add")}
                  >
                    <FontAwesomeIcon icon={["fas", "plus"]} className="text-xs" />
                  </button>
                </div>
              ))}
            </div>
          </div>
        </div>
      </div>

      {/* Right: detail / disclosed body */}
      {selected && (
        <div className="w-96 border-l border-border-theme flex flex-col overflow-hidden bg-gray-50/50">
          <div className="px-6 py-5 border-b border-border-theme flex items-start justify-between">
            <div>
              <div className="text-lg font-semibold text-text-base">{selected.name}</div>
              <div className="text-xs text-text-secondary mt-0.5">
                {selected.id} · {originLabel(selected.origin)}
                {selected.version ? ` · v${selected.version}` : ""}
              </div>
            </div>
            <button
              onClick={() => {
                setSelected(null);
                setActivation(null);
              }}
              className="text-text-secondary hover:text-text-base"
            >
              <FontAwesomeIcon icon={["fas", "xmark"]} />
            </button>
          </div>
          <div className="flex-1 overflow-y-auto px-6 py-4">
            <div className="text-xs font-medium text-text-secondary uppercase tracking-wide mb-2">
              {t("skillsView.triggerPhrases", { count: selected.triggers.length })}
            </div>
            <div className="flex flex-wrap gap-1.5 mb-5">
              {selected.triggers.map((t) => (
                <span key={t} className="text-[11px] bg-white border border-border-theme rounded-full px-2 py-0.5 text-text-secondary">
                  {t}
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
    </div>
  );
}
