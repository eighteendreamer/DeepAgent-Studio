/*
import { useEffect, useMemo, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";

type PluginSource = "openai" | "workspace" | "personal" | "marketplace";
type PluginCategory =
  | "Featured"
  | "Creativity"
  | "Data & Analytics"
  | "Developer Tools"
  | "Education & Research"
  | "Productivity"
  | "Security";

interface PluginItem {
  id: string;
  name: string;
  shortName?: string;
  developer: string;
  description: string;
  longDescription: string;
  source: PluginSource;
  category: PluginCategory;
  version: string;
  website?: string;
  privacy?: string;
  terms?: string;
  capabilities: string[];
  skillCount: number;
  permissions: string[];
  icon: IconProp;
  iconClass: string;
  accentClass: string;
}

interface MarketplaceDraft {
  source: string;
  gitRef: string;
  sparsePath: string;
}

interface CreateDraft {
  name: string;
  description: string;
  directory: string;
}

const STORAGE_KEY = "deepagent:plugins-view-state";

const basePlugins: PluginItem[] = [
  {
    id: "browser",
    name: "Browser",
    developer: "DeepAgent",
    description: "Control the in-app browser with DeepAgent",
    longDescription:
      "Browser lets DeepAgent open and control the in-app browser, mainly for local development pages and files. Use it to navigate, inspect, click, type, and take screenshots while testing pages inside DeepAgent Studio.",
    source: "openai",
    category: "Featured",
    version: "26.616.51431",
    website: "https://developers.openai.com/codex",
    privacy: "https://openai.com/policies/privacy-policy",
    terms: "https://openai.com/policies/terms-of-use",
    capabilities: ["Interactive", "Read", "Write"],
    skillCount: 1,
    permissions: ["浏览器控制", "截图", "本地页面访问"],
    icon: ["fas", "arrow-pointer"],
    iconClass: "bg-white text-text-base border border-border-theme",
    accentClass: "from-blue-100 via-indigo-100 to-violet-100",
  },
  {
    id: "computer-use",
    name: "Computer Use",
    developer: "DeepAgent",
    description: "Control Windows apps from DeepAgent",
    longDescription:
      "Computer Use is the desktop interaction plugin placeholder. It will coordinate screen understanding, mouse/keyboard actions, and permission prompts through the system tool layer.",
    source: "openai",
    category: "Featured",
    version: "0.1.0",
    capabilities: ["Interactive", "Read", "Write"],
    skillCount: 2,
    permissions: ["屏幕读取", "鼠标键盘", "应用窗口控制"],
    icon: ["fas", "desktop"],
    iconClass: "bg-gradient-to-br from-cyan-400 to-violet-500 text-white",
    accentClass: "from-cyan-100 via-blue-100 to-fuchsia-100",
  },
  {
    id: "office-agent",
    name: "Office Agent",
    developer: "DeepAgent Studio",
    description: "Create, read, preview, and transform Office files",
    longDescription:
      "Office Agent groups the built-in document, spreadsheet, preview, and meeting workflows. It should call DeepAgent tools and managed runtimes instead of asking users to install Python or Node packages.",
    source: "workspace",
    category: "Productivity",
    version: "0.2.0",
    capabilities: ["Read", "Write", "Managed runtime"],
    skillCount: 4,
    permissions: ["文件读取", "文件写入", "本地模型"],
    icon: ["far", "file-lines"],
    iconClass: "bg-blue-50 text-blue-600",
    accentClass: "from-blue-50 via-sky-100 to-emerald-50",
  },
  {
    id: "meeting-recorder",
    name: "Meeting Recorder",
    developer: "DeepAgent Studio",
    description: "Record, transcribe, and generate meeting minutes",
    longDescription:
      "Meeting Recorder is the office recording plugin placeholder. It manages local recording, speech model checks, transcription, minutes generation, and Word export.",
    source: "workspace",
    category: "Productivity",
    version: "0.2.0",
    capabilities: ["Audio", "Read", "Write"],
    skillCount: 1,
    permissions: ["麦克风", "本地模型", "文件写入"],
    icon: ["fas", "microphone"],
    iconClass: "bg-rose-50 text-rose-600",
    accentClass: "from-rose-50 via-orange-50 to-amber-50",
  },
  {
    id: "chrome",
    name: "Chrome",
    developer: "openai-api-curated",
    description: "Control Chrome with DeepAgent",
    longDescription:
      "Chrome connects browser automation to DeepAgent so the model can inspect pages, click, type, and verify UI behavior after permission approval.",
    source: "marketplace",
    category: "Featured",
    version: "1.4.2",
    capabilities: ["Interactive", "Read", "Write"],
    skillCount: 1,
    permissions: ["浏览器控制", "网页读取"],
    icon: ["fab", "chrome"],
    iconClass: "bg-white text-green-600 border border-border-theme",
    accentClass: "from-green-50 via-yellow-50 to-red-50",
  },
  {
    id: "remotion",
    name: "Remotion",
    developer: "openai-api-curated",
    description: "Create motion graphics from prompts",
    longDescription:
      "Remotion helps generate scripted motion graphics and video-oriented web scenes. This is a marketplace placeholder for future plugin installation.",
    source: "marketplace",
    category: "Creativity",
    version: "0.8.5",
    capabilities: ["Write", "Render"],
    skillCount: 2,
    permissions: ["文件写入", "命令执行"],
    icon: ["fas", "play"],
    iconClass: "bg-blue-50 text-blue-600",
    accentClass: "from-sky-50 via-blue-100 to-indigo-50",
  },
  {
    id: "hyperframe",
    name: "HyperFrame",
    shortName: "HyperFra...",
    developer: "openai-api-curated",
    description: "Write HTML, render video",
    longDescription:
      "HyperFrame is a creative rendering plugin candidate for HTML-based animation and export workflows.",
    source: "marketplace",
    category: "Creativity",
    version: "0.6.1",
    capabilities: ["Write", "Render"],
    skillCount: 2,
    permissions: ["文件写入", "渲染运行时"],
    icon: ["fas", "code"],
    iconClass: "bg-emerald-50 text-emerald-600",
    accentClass: "from-emerald-50 via-cyan-50 to-teal-50",
  },
  {
    id: "mixpanel",
    name: "Mixpanel Analytics",
    shortName: "Mixpanel...",
    developer: "openai-api-curated",
    description: "Analyze Mixpanel data with DeepAgent",
    longDescription:
      "Mixpanel Analytics is a data connector placeholder for querying product analytics and preparing reports.",
    source: "marketplace",
    category: "Data & Analytics",
    version: "2.1.0",
    capabilities: ["Read", "Analyze"],
    skillCount: 1,
    permissions: ["网络访问", "API Key"],
    icon: ["fas", "table"],
    iconClass: "bg-violet-50 text-violet-600",
    accentClass: "from-violet-50 via-purple-50 to-indigo-50",
  },
  {
    id: "game-studio",
    name: "Game Studio",
    shortName: "Game St...",
    developer: "openai-api-curated",
    description: "Design, prototype, and ship games",
    longDescription:
      "Game Studio bundles workflows for game planning, UI prototyping, testing, and asset iteration.",
    source: "marketplace",
    category: "Developer Tools",
    version: "0.9.0",
    capabilities: ["Write", "Test"],
    skillCount: 3,
    permissions: ["文件写入", "命令执行"],
    icon: ["fas", "cube"],
    iconClass: "bg-gray-50 text-text-secondary",
    accentClass: "from-gray-50 via-slate-100 to-blue-50",
  },
  {
    id: "superpowers",
    name: "Superpowers",
    shortName: "Superpo...",
    developer: "openai-api-curated",
    description: "Planning, TDD, debugging, and more",
    longDescription:
      "Superpowers collects engineering workflows such as planning, test-driven development, debugging, and review.",
    source: "marketplace",
    category: "Developer Tools",
    version: "1.0.0",
    capabilities: ["Read", "Write", "Test"],
    skillCount: 14,
    permissions: ["文件读取", "文件写入", "命令执行"],
    icon: ["fas", "link"],
    iconClass: "bg-white text-text-base border border-border-theme",
    accentClass: "from-gray-50 via-blue-50 to-indigo-50",
  },
  {
    id: "circleci",
    name: "CircleCI",
    developer: "openai-api-curated",
    description: "Build, test, and deploy anything",
    longDescription:
      "CircleCI is a CI/CD integration placeholder for reading build results and triggering controlled pipeline workflows.",
    source: "marketplace",
    category: "Developer Tools",
    version: "1.3.0",
    capabilities: ["Read", "Write"],
    skillCount: 1,
    permissions: ["网络访问", "API Key"],
    icon: ["fas", "circle-check"],
    iconClass: "bg-gray-100 text-gray-700",
    accentClass: "from-gray-50 via-zinc-50 to-slate-100",
  },
  {
    id: "sentry",
    name: "Sentry",
    developer: "openai-api-curated",
    description: "Inspect recent Sentry issues and releases",
    longDescription:
      "Sentry connects issue tracking and release health to DeepAgent so the model can summarize and triage incidents.",
    source: "marketplace",
    category: "Developer Tools",
    version: "1.2.8",
    capabilities: ["Read", "Analyze"],
    skillCount: 1,
    permissions: ["网络访问", "API Key"],
    icon: ["fas", "triangle-exclamation"],
    iconClass: "bg-purple-100 text-purple-700",
    accentClass: "from-purple-50 via-violet-50 to-indigo-50",
  },
  {
    id: "security-review",
    name: "Security Review",
    developer: "DeepAgent Studio",
    description: "Review plugin permissions and installation risks",
    longDescription:
      "Security Review is a planned built-in plugin for scanning manifests, reviewing install scripts, and explaining plugin permissions before installation.",
    source: "workspace",
    category: "Security",
    version: "0.1.0",
    capabilities: ["Read", "Analyze"],
    skillCount: 1,
    permissions: ["文件读取"],
    icon: ["fas", "shield-halved"],
    iconClass: "bg-emerald-50 text-emerald-700",
    accentClass: "from-emerald-50 via-green-50 to-lime-50",
  },
];

const defaultInstalledIds = ["browser", "office-agent", "meeting-recorder"];
const categoryOrder: PluginCategory[] = [
  "Featured",
  "Creativity",
  "Data & Analytics",
  "Developer Tools",
  "Education & Research",
  "Productivity",
  "Security",
];

const sourceTabs: Array<{ id: PluginSource | "all"; label: string }> = [
  { id: "openai", label: "由 DeepAgent 提供" },
  { id: "workspace", label: "工作区插件" },
  { id: "personal", label: "个人" },
];

const categoryLabels: Record<PluginCategory, string> = {
  "Featured": "精选",
  "Creativity": "创意",
  "Data & Analytics": "数据与分析",
  "Developer Tools": "开发者工具",
  "Education & Research": "教育与研究",
  "Productivity": "效率",
  "Security": "安全",
};

function readPluginState(): { installedIds: string[]; personalPlugins: PluginItem[]; marketplaces: MarketplaceDraft[] } {
  if (typeof window === "undefined") {
    return { installedIds: defaultInstalledIds, personalPlugins: [], marketplaces: [] };
  }
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { installedIds: defaultInstalledIds, personalPlugins: [], marketplaces: [] };
    const parsed = JSON.parse(raw) as {
      installedIds?: unknown;
      personalPlugins?: unknown;
      marketplaces?: unknown;
    };
    return {
      installedIds: Array.isArray(parsed.installedIds)
        ? parsed.installedIds.filter((item): item is string => typeof item === "string")
        : defaultInstalledIds,
      personalPlugins: Array.isArray(parsed.personalPlugins)
        ? parsed.personalPlugins.filter(isPluginItem)
        : [],
      marketplaces: Array.isArray(parsed.marketplaces)
        ? parsed.marketplaces.filter(isMarketplaceDraft)
        : [],
    };
  } catch {
    return { installedIds: defaultInstalledIds, personalPlugins: [], marketplaces: [] };
  }
}

function isMarketplaceDraft(value: unknown): value is MarketplaceDraft {
  if (!value || typeof value !== "object") return false;
  const item = value as Record<string, unknown>;
  return (
    typeof item.source === "string" &&
    typeof item.gitRef === "string" &&
    typeof item.sparsePath === "string"
  );
}

function isPluginItem(value: unknown): value is PluginItem {
  if (!value || typeof value !== "object") return false;
  const item = value as Record<string, unknown>;
  return typeof item.id === "string" && typeof item.name === "string" && typeof item.description === "string";
}

function sourceLabel(source: PluginSource): string {
  switch (source) {
    case "openai":
      return "由 DeepAgent 提供";
    case "workspace":
      return "工作区插件";
    case "personal":
      return "个人";
    case "marketplace":
      return "插件市场";
  }
}

function pluginSearchText(plugin: PluginItem): string {
  return [
    plugin.id,
    plugin.name,
    plugin.shortName,
    plugin.developer,
    plugin.description,
    plugin.category,
    plugin.capabilities.join(" "),
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
}

function slugify(input: string): string {
  return (
    input
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9\u4e00-\u9fa5]+/g, "-")
      .replace(/^-+|-+$/g, "") || `plugin-${Date.now()}`
  );
}

function createPersonalPlugin(draft: CreateDraft): PluginItem {
  const name = draft.name.trim();
  const description = draft.description.trim() || "Personal plugin created from DeepAgent Studio.";
  return {
    id: slugify(name),
    name,
    developer: "Personal",
    description,
    longDescription:
      "这个插件会通过插件创建 Skill 生成 manifest、skills、MCP 或工具目录。当前页面先完成管理入口占位，后续接入 PluginService 后会真正写入插件目录。",
    source: "personal",
    category: "Developer Tools",
    version: "0.1.0",
    capabilities: ["Skill", "Tool"],
    skillCount: 1,
    permissions: ["按创建向导配置"],
    icon: ["fas", "puzzle-piece"],
    iconClass: "bg-gray-100 text-text-base",
    accentClass: "from-gray-50 via-blue-50 to-indigo-50",
  };
}

function PluginIcon({ plugin, size = "md" }: { plugin: PluginItem; size?: "sm" | "md" | "lg" }) {
  const className =
    size === "lg" ? "w-16 h-16 rounded-2xl text-3xl" : size === "sm" ? "w-9 h-9 rounded-xl" : "w-12 h-12 rounded-xl";
  return (
    <div className={`${className} ${plugin.iconClass} flex items-center justify-center flex-shrink-0`}>
      <FontAwesomeIcon icon={plugin.icon} />
    </div>
  );
}

function ExternalLink({ href }: { href?: string }) {
  if (!href) return <span className="text-text-secondary">-</span>;
  return (
    <a href={href} target="_blank" rel="noreferrer" className="text-text-base hover:text-primary">
      <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="text-[12px]" />
    </a>
  );
}

export function PluginsView() {
  const initial = useMemo(readPluginState, []);
  const [installedIds, setInstalledIds] = useState<string[]>(initial.installedIds);
  const [personalPlugins, setPersonalPlugins] = useState<PluginItem[]>(initial.personalPlugins);
  const [marketplaces, setMarketplaces] = useState<MarketplaceDraft[]>(initial.marketplaces);
  const [query, setQuery] = useState("");
  const [sourceTab, setSourceTab] = useState<PluginSource | "all">("openai");
  const [categoryFilter, setCategoryFilter] = useState<PluginCategory | "all">("all");
  const [filterOpen, setFilterOpen] = useState(false);
  const [actionsOpen, setActionsOpen] = useState(false);
  const [detailId, setDetailId] = useState<string | null>(null);
  const [marketplaceDialogOpen, setMarketplaceDialogOpen] = useState(false);
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [marketplaceDraft, setMarketplaceDraft] = useState<MarketplaceDraft>({
    source: "",
    gitRef: "main",
    sparsePath: "",
  });
  const [createDraft, setCreateDraft] = useState<CreateDraft>({
    name: "",
    description: "",
    directory: "",
  });
  const actionsRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLDivElement>(null);

  const plugins = useMemo(() => [...basePlugins, ...personalPlugins], [personalPlugins]);
  const installedSet = useMemo(() => new Set(installedIds), [installedIds]);
  const detailPlugin = detailId ? plugins.find((plugin) => plugin.id === detailId) ?? null : null;

  useEffect(() => {
    if (typeof window === "undefined") return;
    window.localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ installedIds, personalPlugins, marketplaces })
    );
  }, [installedIds, marketplaces, personalPlugins]);

  useEffect(() => {
    const onPointerDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (actionsRef.current && !actionsRef.current.contains(target)) setActionsOpen(false);
      if (filterRef.current && !filterRef.current.contains(target)) setFilterOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, []);

  const searchedPlugins = useMemo(() => {
    const q = query.trim().toLowerCase();
    return plugins.filter((plugin) => {
      const matchesQuery = !q || pluginSearchText(plugin).includes(q);
      const matchesSource =
        sourceTab === "all" ||
        plugin.source === sourceTab ||
        (sourceTab === "openai" && plugin.source === "marketplace");
      const matchesCategory = categoryFilter === "all" || plugin.category === categoryFilter;
      return matchesQuery && matchesSource && matchesCategory;
    });
  }, [categoryFilter, plugins, query, sourceTab]);

  const installedPlugins = useMemo(
    () => plugins.filter((plugin) => installedSet.has(plugin.id)),
    [installedSet, plugins]
  );

  const groupedPlugins = useMemo(() => {
    return categoryOrder
      .map((category) => ({
        category,
        plugins: searchedPlugins.filter((plugin) => plugin.category === category),
      }))
      .filter((group) => group.plugins.length > 0);
  }, [searchedPlugins]);

  const installPlugin = (id: string) => {
    setInstalledIds((current) => (current.includes(id) ? current : [...current, id]));
  };

  const uninstallPlugin = (id: string) => {
    setInstalledIds((current) => current.filter((item) => item !== id));
    if (detailId === id) setDetailId(null);
  };

  const submitMarketplace = () => {
    const source = marketplaceDraft.source.trim();
    if (!source) return;
    setMarketplaces((current) => [...current, { ...marketplaceDraft, source }]);
    setMarketplaceDraft({ source: "", gitRef: "main", sparsePath: "" });
    setMarketplaceDialogOpen(false);
  };

  const submitCreatePlugin = () => {
    if (!createDraft.name.trim()) return;
    const plugin = createPersonalPlugin(createDraft);
    setPersonalPlugins((current) => [...current.filter((item) => item.id !== plugin.id), plugin]);
    installPlugin(plugin.id);
    setCreateDraft({ name: "", description: "", directory: "" });
    setCreateDialogOpen(false);
    setSourceTab("personal");
    setDetailId(plugin.id);
  };

  return (
    <div className="w-full h-full bg-white overflow-hidden relative">
      <div className="absolute right-8 top-6 flex items-center gap-3 z-20">
        <div className="relative" ref={actionsRef}>
          <button
            type="button"
            onClick={() => setActionsOpen((open) => !open)}
            className="h-8 rounded-xl border border-border-theme bg-white px-3 text-text-base hover:bg-gray-50 transition-colors flex items-center gap-2"
            title="添加"
          >
            <FontAwesomeIcon icon={["fas", "plus"]} className="text-[13px]" />
            <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary" />
          </button>
          {actionsOpen && (
            <div className="absolute right-0 top-full mt-2 w-52 rounded-xl border border-border-theme bg-white py-2 shadow-[0_10px_35px_rgba(15,23,42,0.14)]">
              <button
                type="button"
                onClick={() => {
                  setActionsOpen(false);
                  setCreateDialogOpen(true);
                }}
                className="w-full px-4 py-2.5 text-left text-[13px] text-text-base hover:bg-gray-50 flex items-center"
              >
                <FontAwesomeIcon icon={["fas", "puzzle-piece"]} className="w-4 mr-3 text-text-secondary" />
                创建插件
              </button>
              <button
                type="button"
                onClick={() => {
                  setActionsOpen(false);
                  setMarketplaceDialogOpen(true);
                }}
                className="w-full px-4 py-2.5 text-left text-[13px] text-text-base hover:bg-gray-50 flex items-center"
              >
                <FontAwesomeIcon icon={["fas", "plus"]} className="w-4 mr-3 text-text-secondary" />
                添加插件市场
              </button>
            </div>
          )}
        </div>
        <button
          type="button"
          className="h-8 w-8 rounded-lg text-text-secondary hover:text-text-base hover:bg-gray-50 transition-colors"
          title="刷新"
        >
          <FontAwesomeIcon icon={["fas", "rotate-right"]} className="text-[13px]" />
        </button>
      </div>

      {detailPlugin ? (
        <PluginDetail
          plugin={detailPlugin}
          installed={installedSet.has(detailPlugin.id)}
          onBack={() => setDetailId(null)}
          onInstall={() => installPlugin(detailPlugin.id)}
          onUninstall={() => uninstallPlugin(detailPlugin.id)}
        />
      ) : (
        <div className="h-full overflow-y-scroll custom-scrollbar">
          <div className="mx-auto w-full max-w-[920px] px-8 pb-16 pt-20">
            <div className="mb-6">
              <h1 className="text-3xl font-semibold text-text-base mb-2">插件</h1>
              <p className="text-[15px] text-text-secondary">在常用工具中使用 DeepAgent</p>
            </div>

            <div className="mb-9 flex items-center gap-3">
              <div className="flex h-11 flex-1 items-center rounded-full border border-border-theme bg-white px-4 focus-within:border-gray-300 transition-colors">
                <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className="mr-3 text-text-secondary" />
                <input
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="搜索插件和技能"
                  className="w-full bg-transparent text-[14px] text-text-base outline-none"
                />
              </div>
              <div className="relative" ref={filterRef}>
                <button
                  type="button"
                  onClick={() => setFilterOpen((open) => !open)}
                  className={`h-11 w-11 rounded-full border border-border-theme flex items-center justify-center transition-colors ${
                    filterOpen || categoryFilter !== "all"
                      ? "bg-gray-50 text-text-base"
                      : "bg-white text-text-secondary hover:text-text-base"
                  }`}
                  title="筛选分类"
                >
                  <FontAwesomeIcon icon={["fas", "sliders"]} />
                </button>
                {filterOpen && (
                  <div className="absolute right-0 top-full mt-2 w-60 rounded-xl border border-border-theme bg-white py-2 shadow-[0_12px_40px_rgba(15,23,42,0.14)]">
                    {(["all", ...categoryOrder] as Array<PluginCategory | "all">).map((category) => (
                      <button
                        key={category}
                        type="button"
                        onClick={() => {
                          setCategoryFilter(category);
                          setFilterOpen(false);
                        }}
                        className="w-full px-4 py-2.5 text-left text-[13px] text-text-base hover:bg-gray-50 flex items-center justify-between"
                      >
                        <span>{category === "all" ? "全部" : categoryLabels[category as PluginCategory]}</span>
                        {categoryFilter === category && (
                          <FontAwesomeIcon icon={["fas", "check"]} className="text-text-secondary text-[12px]" />
                        )}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            </div>

            <div className="mb-8">
              <div className="flex items-center justify-between border-b border-border-theme pb-3">
                <h2 className="text-[17px] font-semibold text-text-base">已添加</h2>
                <button
                  type="button"
                  onClick={() => setSourceTab("all")}
                  className="text-[13px] text-text-secondary hover:text-text-base"
                >
                  管理
                </button>
              </div>

              {installedPlugins.length === 0 ? (
                <div className="py-8 text-[13px] text-text-secondary">暂无已添加插件</div>
              ) : (
                <div className="grid grid-cols-1 gap-x-14 gap-y-4 py-4 md:grid-cols-2">
                  {installedPlugins.map((plugin) => (
                    <PluginRow
                      key={plugin.id}
                      plugin={plugin}
                      installed
                      onOpen={() => setDetailId(plugin.id)}
                      onInstall={() => installPlugin(plugin.id)}
                      onUninstall={() => uninstallPlugin(plugin.id)}
                    />
                  ))}
                </div>
              )}
            </div>

            <div className="mb-7 flex items-center gap-5">
              {sourceTabs.map((tab) => (
                <button
                  key={tab.id}
                  type="button"
                  onClick={() => setSourceTab(tab.id)}
                  className={`rounded-lg px-3 py-1.5 text-[13px] transition-colors font-medium ${
                    sourceTab === tab.id
                      ? "bg-gray-100 text-text-base"
                      : "text-text-secondary hover:text-text-base hover:bg-gray-50"
                  }`}
                >
                  {tab.label}
                </button>
              ))}
            </div>

            {groupedPlugins.length === 0 ? (
              <div className="rounded-lg border border-border-theme px-5 py-8 text-center text-[13px] text-text-secondary">
                没有匹配的插件
              </div>
            ) : (
              groupedPlugins.map((group) => (
                <section key={group.category} className="mb-9">
                  <h2 className="border-b border-border-theme pb-3 text-[17px] font-semibold text-text-base">
                    {categoryLabels[group.category]}
                  </h2>
                  <div className="grid grid-cols-1 gap-x-14 gap-y-4 py-5 md:grid-cols-2">
                    {group.plugins.map((plugin) => (
                      <PluginRow
                        key={plugin.id}
                        plugin={plugin}
                        installed={installedSet.has(plugin.id)}
                        onOpen={() => setDetailId(plugin.id)}
                        onInstall={() => installPlugin(plugin.id)}
                        onUninstall={() => uninstallPlugin(plugin.id)}
                      />
                    ))}
                  </div>
                </section>
              ))
            )}

            {marketplaces.length > 0 && (
              <div className="mt-10 border-t border-border-theme pt-5">
                <div className="mb-3 text-[13px] font-medium text-text-base">已添加插件市场</div>
                <div className="space-y-2">
                  {marketplaces.map((marketplace, index) => (
                    <div key={`${marketplace.source}-${index}`} className="flex items-center justify-between rounded-lg bg-gray-50 px-3 py-2 text-[12px] text-text-secondary">
                      <span className="truncate">{marketplace.source}</span>
                      <button
                        type="button"
                        onClick={() => setMarketplaces((current) => current.filter((_, i) => i !== index))}
                        className="ml-3 text-text-secondary hover:text-red-500"
                      >
                        删除
                      </button>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>
      )}

      {marketplaceDialogOpen && (
        <AddMarketplaceDialog
          draft={marketplaceDraft}
          onChange={setMarketplaceDraft}
          onClose={() => setMarketplaceDialogOpen(false)}
          onSubmit={submitMarketplace}
        />
      )}

      {createDialogOpen && (
        <CreatePluginDialog
          draft={createDraft}
          onChange={setCreateDraft}
          onClose={() => setCreateDialogOpen(false)}
          onSubmit={submitCreatePlugin}
        />
      )}
    </div>
  );
}

function PluginRow({
  plugin,
  installed,
  onOpen,
  onInstall,
  onUninstall,
}: {
  plugin: PluginItem;
  installed: boolean;
  onOpen: () => void;
  onInstall: () => void;
  onUninstall: () => void;
}) {
  return (
    <div className="group flex min-w-0 items-center gap-3">
      <button type="button" onClick={onOpen} className="flex min-w-0 flex-1 items-center gap-3 text-left">
        <PluginIcon plugin={plugin} size="sm" />
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-baseline gap-2">
            <span className="truncate text-[14px] font-semibold text-text-base">
              {plugin.shortName ?? plugin.name}
            </span>
            {plugin.source === "marketplace" && (
              <span className="truncate text-[12px] text-text-secondary">{plugin.developer}</span>
            )}
          </div>
          <div className="mt-0.5 truncate text-[13px] text-text-secondary">{plugin.description}</div>
        </div>
      </button>
      {installed ? (
        <button
          type="button"
          onClick={onUninstall}
          className="rounded-full border border-border-theme px-3 py-1 text-[12px] text-text-secondary opacity-0 transition-all hover:border-red-200 hover:bg-red-50 hover:text-red-500 group-hover:opacity-100"
        >
          删除
        </button>
      ) : (
        <button
          type="button"
          onClick={onInstall}
          className="rounded-full border border-border-theme px-3 py-1 text-[12px] text-text-base hover:bg-gray-50"
        >
          添加插件
        </button>
      )}
    </div>
  );
}

function PluginDetail({
  plugin,
  installed,
  onBack,
  onInstall,
  onUninstall,
}: {
  plugin: PluginItem;
  installed: boolean;
  onBack: () => void;
  onInstall: () => void;
  onUninstall: () => void;
}) {
  return (
    <div className="h-full overflow-y-scroll custom-scrollbar">
      <div className="mx-auto w-full max-w-[880px] px-8 pb-16 pt-8">
        <div className="mb-12 flex items-center gap-3 text-[14px]">
          <button type="button" onClick={onBack} className="text-text-secondary hover:text-text-base">
            插件
          </button>
          <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] text-text-secondary" />
          <span className="text-text-base">{plugin.name}</span>
        </div>

        <div className="mb-7 flex items-center justify-between gap-6">
          <div className="flex min-w-0 items-center gap-5">
            <PluginIcon plugin={plugin} size="lg" />
            <div className="min-w-0">
              <h1 className="truncate text-3xl font-semibold text-text-base">{plugin.name}</h1>
              <p className="mt-1 text-[15px] text-text-secondary">{plugin.description}</p>
            </div>
          </div>
          <div className="flex items-center gap-3">
            <button
              type="button"
              className="h-9 w-9 rounded-lg text-text-secondary hover:bg-gray-50 hover:text-text-base"
              title="更多"
            >
              <FontAwesomeIcon icon={["fas", "ellipsis"]} />
            </button>
            {installed ? (
              <button
                type="button"
                onClick={onUninstall}
                className="rounded-lg bg-text-base px-4 py-2 text-[13px] font-medium text-white hover:bg-black"
              >
                删除插件
              </button>
            ) : (
              <button
                type="button"
                onClick={onInstall}
                className="rounded-lg bg-text-base px-4 py-2 text-[13px] font-medium text-white hover:bg-black"
              >
                添加插件
              </button>
            )}
          </div>
        </div>

        <div className={`mb-8 rounded-lg bg-gradient-to-br ${plugin.accentClass} px-8 py-9`}>
          <div className="mx-auto flex w-fit max-w-full items-center rounded-xl bg-white/75 px-4 py-3 text-[14px] text-text-base shadow-sm">
            <span className="mr-2 font-semibold text-primary">{plugin.name}</span>
            <span className="truncate">在 DeepAgent 中使用此插件</span>
            <FontAwesomeIcon icon={["fas", "arrow-right"]} className="ml-3 text-text-secondary" />
          </div>
        </div>

        <p className="mb-12 max-w-[760px] text-[14px] leading-7 text-text-secondary">
          {plugin.longDescription}
        </p>

        <section className="mb-12">
          <h2 className="mb-5 text-[17px] font-semibold text-text-base">技能 {plugin.skillCount}</h2>
          <div className="grid grid-cols-[200px_1fr] gap-x-10 gap-y-4 text-[14px]">
            <div className="font-semibold text-text-base">
              {plugin.capabilities[0] ?? "插件能力"}
            </div>
            <div className="truncate text-text-secondary">
              {plugin.longDescription}
            </div>
          </div>
        </section>

        <section>
          <h2 className="mb-6 text-[17px] font-semibold text-text-base">信息</h2>
          <div className="grid grid-cols-[160px_1fr] gap-x-12 gap-y-6 text-[14px]">
            <div className="text-text-secondary">功能</div>
            <div className="text-text-base">{plugin.capabilities.join(", ")}</div>
            <div className="text-text-secondary">权限</div>
            <div className="text-text-base">{plugin.permissions.join(", ")}</div>
            <div className="text-text-secondary">开发者</div>
            <div className="text-text-base">{plugin.developer}</div>
            <div className="text-text-secondary">类别</div>
            <div className="text-text-base">{categoryLabels[plugin.category]}</div>
            <div className="text-text-secondary">来源</div>
            <div className="text-text-base">{sourceLabel(plugin.source)}</div>
            <div className="text-text-secondary">网站</div>
            <ExternalLink href={plugin.website} />
            <div className="text-text-secondary">版本</div>
            <div className="text-text-base">{plugin.version}</div>
            <div className="text-text-secondary">隐私政策</div>
            <ExternalLink href={plugin.privacy} />
            <div className="text-text-secondary">服务条款</div>
            <ExternalLink href={plugin.terms} />
          </div>
        </section>
      </div>
    </div>
  );
}

function AddMarketplaceDialog({
  draft,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: MarketplaceDraft;
  onChange: (draft: MarketplaceDraft) => void;
  onClose: () => void;
  onSubmit: () => void;
}) {
  return (
    <div className="absolute inset-0 z-40 flex items-center justify-center bg-black/20 px-6">
      <div className="w-full max-w-[680px] rounded-2xl bg-white p-6 shadow-[0_25px_80px_rgba(15,23,42,0.22)]">
        <div className="mb-7 flex items-start justify-between">
          <div>
            <h2 className="text-2xl font-semibold text-text-base">添加插件市场</h2>
            <p className="mt-2 text-[13px] text-text-secondary">
              从 GitHub 仓库、Git URL 或本地文件夹添加。
              <a href="#" className="ml-2 text-primary hover:underline">了解更多</a>
            </p>
          </div>
          <button type="button" onClick={onClose} className="text-text-secondary hover:text-text-base">
            <FontAwesomeIcon icon={["fas", "xmark"]} />
          </button>
        </div>

        <div className="space-y-4">
          <Field label="来源">
            <input
              value={draft.source}
              onChange={(event) => onChange({ ...draft, source: event.target.value })}
              placeholder="openai/plugins 或 git@github.com:org/repo.git"
              className="w-full rounded-lg border border-border-theme px-3 py-2 text-[14px] outline-none focus:border-primary"
            />
          </Field>
          <Field label="Git 引用">
            <input
              value={draft.gitRef}
              onChange={(event) => onChange({ ...draft, gitRef: event.target.value })}
              placeholder="主分支"
              className="w-full rounded-lg border border-border-theme px-3 py-2 text-[14px] outline-none focus:border-primary"
            />
          </Field>
          <Field label="稀疏路径">
            <textarea
              value={draft.sparsePath}
              onChange={(event) => onChange({ ...draft, sparsePath: event.target.value })}
              placeholder="plugins/codex"
              className="min-h-24 w-full resize-y rounded-lg border border-border-theme px-3 py-2 text-[14px] outline-none focus:border-primary"
            />
          </Field>
        </div>

        <div className="mt-8 flex justify-end gap-3">
          <button type="button" onClick={onClose} className="rounded-lg border border-border-theme px-5 py-2 text-[13px] text-text-base hover:bg-gray-50">
            取消
          </button>
          <button
            type="button"
            onClick={onSubmit}
            disabled={!draft.source.trim()}
            className="rounded-lg bg-text-base px-5 py-2 text-[13px] font-medium text-white hover:bg-black disabled:cursor-not-allowed disabled:opacity-40"
          >
            添加市场
          </button>
        </div>
      </div>
    </div>
  );
}

function CreatePluginDialog({
  draft,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: CreateDraft;
  onChange: (draft: CreateDraft) => void;
  onClose: () => void;
  onSubmit: () => void;
}) {
  return (
    <div className="absolute inset-0 z-40 flex items-center justify-center bg-black/20 px-6">
      <div className="w-full max-w-[680px] rounded-2xl bg-white p-6 shadow-[0_25px_80px_rgba(15,23,42,0.22)]">
        <div className="mb-7 flex items-start justify-between">
          <div>
            <h2 className="text-2xl font-semibold text-text-base">创建插件</h2>
            <p className="mt-2 text-[13px] leading-6 text-text-secondary">
              创建入口会交给插件创建 Skill 生成 manifest、skills、MCP 或工具模板。
            </p>
          </div>
          <button type="button" onClick={onClose} className="text-text-secondary hover:text-text-base">
            <FontAwesomeIcon icon={["fas", "xmark"]} />
          </button>
        </div>

        <div className="space-y-4">
          <Field label="插件名称">
            <input
              value={draft.name}
              onChange={(event) => onChange({ ...draft, name: event.target.value })}
              placeholder="例如 Office Helper"
              className="w-full rounded-lg border border-border-theme px-3 py-2 text-[14px] outline-none focus:border-primary"
            />
          </Field>
          <Field label="插件说明">
            <textarea
              value={draft.description}
              onChange={(event) => onChange({ ...draft, description: event.target.value })}
              placeholder="这个插件提供哪些技能、工具或 MCP 能力"
              className="min-h-20 w-full resize-y rounded-lg border border-border-theme px-3 py-2 text-[14px] outline-none focus:border-primary"
            />
          </Field>
          <Field label="保存目录">
            <input
              value={draft.directory}
              onChange={(event) => onChange({ ...draft, directory: event.target.value })}
              placeholder=".deepagent/plugins/office-helper"
              className="w-full rounded-lg border border-border-theme px-3 py-2 text-[14px] outline-none focus:border-primary"
            />
          </Field>
        </div>

        <div className="mt-6 rounded-lg bg-gray-50 px-4 py-3 text-[12px] leading-5 text-text-secondary">
          当前先创建前端管理记录。后续接入后端后，这个按钮会调用插件创建 Skill 生成插件目录，并刷新插件列表。
        </div>

        <div className="mt-8 flex justify-end gap-3">
          <button type="button" onClick={onClose} className="rounded-lg border border-border-theme px-5 py-2 text-[13px] text-text-base hover:bg-gray-50">
            取消
          </button>
          <button
            type="button"
            onClick={onSubmit}
            disabled={!draft.name.trim()}
            className="rounded-lg bg-text-base px-5 py-2 text-[13px] font-medium text-white hover:bg-black disabled:cursor-not-allowed disabled:opacity-40"
          >
            创建插件
          </button>
        </div>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <div className="mb-2 text-[13px] text-text-secondary">{label}</div>
      {children}
    </label>
  );
}
*/

export { PluginsView } from "./PluginsViewReal";
