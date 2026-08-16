import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import {
  addPluginMarketplace,
  createPlugin,
  installPluginFromDir,
  installPluginFromMarketplace,
  installPluginFromZip,
  isTauri,
  listPluginMarketplaceEntries,
  listPluginMarketplaces,
  listPluginOutputStyles,
  listPlugins,
  refreshPluginMarketplace,
  removePluginMarketplace,
  scanPlugin,
  scanPluginMarketplace,
  scanPluginZip,
  setPluginEnabled,
  uninstallPlugin,
  updatePlugin,
} from "../api";
import type {
  AddPluginMarketplaceInput,
  CreatePluginDraft,
  Plugin,
  PluginDiagnosticSeverity,
  PluginMarketplace,
  PluginMarketplaceEntry,
  PluginOutputStyle,
  PluginScanReport,
} from "../types";
import { Button } from "./shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./shadcn/dialog";
import { Input } from "./shadcn/input";
import { Label } from "./shadcn/label";
import { Textarea } from "./shadcn/textarea";
import { ToggleSwitch } from "./ui/ToggleSwitch";

type OriginFilter = "all" | "builtin" | "workspace" | "personal" | "marketplace";

type PendingScanAction = {
  title: string;
  submitLabel: string;
  report: PluginScanReport;
  authenticationHint?: string | null;
  onConfirm: () => Promise<void>;
};

type PendingConfirmAction = {
  title: string;
  message: string;
  submitLabel: string;
  tone?: "danger" | "warning";
  dependents?: Plugin["required_by"];
  onConfirm: () => Promise<void>;
};

const originTabs: Array<{ id: OriginFilter; label: string }> = [
  { id: "all", label: "全部" },
  { id: "builtin", label: "DeepAgent 提供" },
  { id: "workspace", label: "工作区" },
  { id: "personal", label: "个人" },
  { id: "marketplace", label: "市场" },
];

const categoryOrder = [
  "Featured",
  "Productivity",
  "Developer Tools",
  "Data & Analytics",
  "Creativity",
  "Security",
];

const emptyCreateDraft: CreatePluginDraft = {
  name: "",
  description: "",
  directory: "",
  category: "Developer Tools",
};

const emptyMarketplaceDraft: AddPluginMarketplaceInput = {
  name: "",
  source: "",
  git_ref: "main",
  sparse_path: "",
};

const deepSeekHarnessMarketplaceDraft: AddPluginMarketplaceInput = {
  name: "deepseek-harness",
  source: "https://github.com/topics/dsh-plugin",
  git_ref: "main",
  sparse_path: "",
};

const deepSeekHarnessMarketplaceBusyId = "marketplace:deepseek-harness";

export function PluginsView() {
  const [plugins, setPlugins] = useState<Plugin[]>([]);
  const [marketplaces, setMarketplaces] = useState<PluginMarketplace[]>([]);
  const [marketplaceEntries, setMarketplaceEntries] = useState<PluginMarketplaceEntry[]>([]);
  const [outputStyles, setOutputStyles] = useState<PluginOutputStyle[]>([]);
  const [query, setQuery] = useState("");
  const [origin, setOrigin] = useState<OriginFilter>("all");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [marketOpen, setMarketOpen] = useState(false);
  const [createDraft, setCreateDraft] = useState<CreatePluginDraft>(emptyCreateDraft);
  const [marketDraft, setMarketDraft] =
    useState<AddPluginMarketplaceInput>(emptyMarketplaceDraft);
  const [scanDialog, setScanDialog] = useState<PendingScanAction | null>(null);
  const [scanBusy, setScanBusy] = useState(false);
  const [confirmDialog, setConfirmDialog] = useState<PendingConfirmAction | null>(null);
  const [confirmBusy, setConfirmBusy] = useState(false);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      const [pluginRows, marketplaceRows, marketplaceEntryRows] = await Promise.all([
        listPlugins(),
        listPluginMarketplaces(),
        listPluginMarketplaceEntries(),
      ]);
      setPlugins(pluginRows);
      setMarketplaces(marketplaceRows);
      setMarketplaceEntries(marketplaceEntryRows);
      setOutputStyles(await listPluginOutputStyles().catch(() => []));
      if (selectedId && !pluginRows.some((plugin) => plugin.id === selectedId)) {
        setSelectedId(null);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const selected = selectedId
    ? plugins.find((plugin) => plugin.id === selectedId) ?? null
    : null;
  const selectedOutputStyles = selected
    ? outputStyles.filter((style) => style.plugin_id === selected.id)
    : [];

  const installed = useMemo(
    () => plugins.filter((plugin) => plugin.installed || plugin.enabled),
    [plugins],
  );

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return plugins.filter((plugin) => {
      const text = [
        plugin.id,
        plugin.name,
        plugin.display_name,
        plugin.description,
        plugin.developer,
        plugin.category,
        plugin.keywords.join(" "),
        plugin.capabilities.join(" "),
      ]
        .filter(Boolean)
        .join(" ")
        .toLowerCase();
      return (
        (!q || text.includes(q)) &&
        (origin === "all" || plugin.origin === origin)
      );
    });
  }, [origin, plugins, query]);

  const grouped = useMemo(() => {
    const known = new Set(categoryOrder);
    const groups = categoryOrder.map((category) => ({
      category,
      plugins: filtered.filter(
        (plugin) => (plugin.category || "Developer Tools") === category,
      ),
    }));
    const other = filtered.filter((plugin) => !known.has(plugin.category || ""));
    if (other.length > 0) groups.push({ category: "Other", plugins: other });
    return groups.filter((group) => group.plugins.length > 0);
  }, [filtered]);

  const applyPluginToggle = async (plugin: Plugin, enabled: boolean) => {
    setBusyId(plugin.id);
    setError(null);
    try {
      await setPluginEnabled(plugin.id, enabled);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyId(null);
    }
  };

  const togglePlugin = async (plugin: Plugin, enabled: boolean) => {
    if (!enabled && plugin.enabled && plugin.required_by.length > 0) {
      setConfirmDialog({
        title: `禁用 ${plugin.display_name}`,
        message: "禁用这个插件后，依赖它的已启用插件会失去对应运行时能力。",
        submitLabel: "禁用插件",
        tone: "warning",
        dependents: plugin.required_by,
        onConfirm: async () => applyPluginToggle(plugin, false),
      });
      return;
    }
    await applyPluginToggle(plugin, enabled);
  };

  const removePlugin = async (plugin: Plugin) => {
    if (plugin.origin === "builtin" || plugin.origin === "workspace") {
      await togglePlugin(plugin, false);
      return;
    }
    setConfirmDialog({
      title: `卸载 ${plugin.display_name}`,
      message:
        plugin.required_by.length > 0
          ? "卸载这个插件会移除依赖插件需要的能力。"
          : "确认卸载这个插件？",
      submitLabel: "卸载插件",
      tone: "danger",
      dependents: plugin.required_by,
      onConfirm: async () => {
        setBusyId(plugin.id);
        setError(null);
        try {
          await uninstallPlugin(plugin.id, false);
          await load();
          setSelectedId(null);
        } catch (err) {
          setError(err instanceof Error ? err.message : String(err));
        } finally {
          setBusyId(null);
        }
      },
    });
  };

  const chooseDirectory = async (): Promise<string | null> => {
    if (isTauri()) {
      const dialog = await import("@tauri-apps/plugin-dialog");
      const selected = await dialog.open({
        title: "选择插件目录",
        directory: true,
        multiple: false,
      });
      return typeof selected === "string" ? selected : null;
    }
    return window.prompt("插件目录路径")?.trim() || null;
  };

  const chooseZip = async (): Promise<string | null> => {
    if (isTauri()) {
      const dialog = await import("@tauri-apps/plugin-dialog");
      const selected = await dialog.open({
        title: "选择插件 zip",
        multiple: false,
        filters: [{ name: "Zip", extensions: ["zip"] }],
      });
      return typeof selected === "string" ? selected : null;
    }
    return window.prompt("插件 zip 路径")?.trim() || null;
  };

  const openScanDialog = (action: PendingScanAction) => {
    setScanDialog(action);
  };

  const confirmScanDialog = async () => {
    if (!scanDialog || scanDialog.report.errors.length > 0) return;
    setScanBusy(true);
    setError(null);
    try {
      await scanDialog.onConfirm();
      setScanDialog(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setScanBusy(false);
    }
  };

  const confirmActionDialog = async () => {
    if (!confirmDialog) return;
    setConfirmBusy(true);
    setError(null);
    try {
      await confirmDialog.onConfirm();
      setConfirmDialog(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setConfirmBusy(false);
    }
  };

  const installFromDirectory = async () => {
    const path = await chooseDirectory();
    if (!path) return;
    setLoading(true);
    setError(null);
    try {
      const report = await scanPlugin(path);
      openScanDialog({
        title: "安装目录插件",
        submitLabel: highRiskCount(report) > 0 ? "继续安装" : "安装插件",
        report,
        onConfirm: async () => {
          const plugin = await installPluginFromDir(path, true);
          await load();
          setSelectedId(plugin.id);
        },
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  const installFromArchive = async () => {
    const path = await chooseZip();
    if (!path) return;
    setLoading(true);
    setError(null);
    try {
      const report = await scanPluginZip(path);
      openScanDialog({
        title: "安装 Zip 插件",
        submitLabel: highRiskCount(report) > 0 ? "继续安装" : "安装插件",
        report,
        onConfirm: async () => {
          const plugin = await installPluginFromZip(path, true);
          await load();
          setSelectedId(plugin.id);
        },
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  const submitCreate = async () => {
    if (!createDraft.name?.trim()) return;
    setError(null);
    try {
      const plugin = await createPlugin(createDraft);
      setCreateOpen(false);
      setCreateDraft(emptyCreateDraft);
      await load();
      setSelectedId(plugin.id);
      setOrigin("personal");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const submitMarketplace = async () => {
    if (!marketDraft.source.trim()) return;
    setError(null);
    try {
      await addPluginMarketplace(marketDraft);
      setMarketOpen(false);
      setMarketDraft(emptyMarketplaceDraft);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const submitDeepSeekHarnessMarketplace = async () => {
    setBusyId(deepSeekHarnessMarketplaceBusyId);
    setError(null);
    try {
      const existing = marketplaces.find(isDeepSeekHarnessMarketplace);
      if (existing) {
        await refreshPluginMarketplace(existing.name);
      } else {
        await addPluginMarketplace(deepSeekHarnessMarketplaceDraft);
      }
      await load();
      setOrigin("marketplace");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyId(null);
    }
  };

  const installMarketplaceEntry = async (entry: PluginMarketplaceEntry) => {
    setBusyId(`${entry.marketplace}:${entry.name}`);
    setError(null);
    try {
      const report = await scanPluginMarketplace(entry.marketplace, entry.name);
      const updating = entry.installed && entry.update_available;
      const authRequired = entry.authentication_required;
      openScanDialog({
        title: `${updating ? "更新" : "安装"} ${entry.display_name}`,
        submitLabel: highRiskCount(report) > 0
          ? updating
            ? "继续更新"
            : "继续安装"
          : updating
            ? "更新插件"
            : "安装插件",
        report,
        authenticationHint: authRequired
          ? entry.authentication_hint ||
            `安装时需要认证: ${entry.policy_authentication || "ON_INSTALL"}`
          : null,
        onConfirm: async () => {
          const plugin = updating
            ? await updatePlugin(
                `${entry.name}@${entry.marketplace}`,
                true,
                authRequired,
              )
            : await installPluginFromMarketplace(
                entry.marketplace,
                entry.name,
                true,
                authRequired,
              );
          await load();
          setSelectedId(plugin.id);
          setOrigin("marketplace");
        },
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyId(null);
    }
  };

  const updateInstalledPlugin = async (plugin: Plugin) => {
    const marketplace = plugin.source.marketplace || plugin.source.name;
    if (plugin.origin !== "marketplace" || !marketplace) return;
    setBusyId(plugin.id);
    setError(null);
    try {
      const report = await scanPluginMarketplace(marketplace, plugin.name);
      const entry = marketplaceEntries.find(
        (item) => item.marketplace === marketplace && item.name === plugin.name,
      );
      const authRequired = Boolean(entry?.authentication_required);
      openScanDialog({
        title: `更新 ${plugin.display_name}`,
        submitLabel: highRiskCount(report) > 0 ? "继续更新" : "更新插件",
        report,
        authenticationHint: authRequired
          ? entry?.authentication_hint ||
            `更新时需要认证: ${entry?.policy_authentication || "ON_INSTALL"}`
          : null,
        onConfirm: async () => {
          const updated = await updatePlugin(plugin.id, true, authRequired);
          await load();
          setSelectedId(updated.id);
          setOrigin("marketplace");
        },
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="h-full w-full overflow-hidden bg-white">
      {selected ? (
        <PluginDetail
          plugin={selected}
          outputStyles={selectedOutputStyles}
          busy={busyId === selected.id}
          onBack={() => setSelectedId(null)}
          onToggle={(enabled) => togglePlugin(selected, enabled)}
          onUpdate={() => updateInstalledPlugin(selected)}
          onRemove={() => removePlugin(selected)}
        />
      ) : (
        <div className="h-full overflow-y-auto custom-scrollbar">
          <div className="mx-auto w-full max-w-[1120px] px-8 py-8">
            <div className="mb-6 flex flex-wrap items-center justify-between gap-3">
              <div>
                <h1 className="text-2xl font-semibold text-text-base">插件</h1>
                <p className="mt-1 text-[13px] text-text-secondary">
                  管理本地、工作区、内置和市场来源的插件能力。
                </p>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <Button variant="outline" onClick={() => void load()}>
                  <FontAwesomeIcon icon={["fas", "rotate"]} className="text-[12px]" />
                  <span>刷新</span>
                </Button>
                <Button variant="outline" onClick={installFromDirectory}>
                  <FontAwesomeIcon icon={["fas", "folder-open"]} className="text-[12px]" />
                  <span>目录安装</span>
                </Button>
                <Button variant="outline" onClick={installFromArchive}>
                  <FontAwesomeIcon icon={["fas", "file-zipper"]} className="text-[12px]" />
                  <span>Zip 安装</span>
                </Button>
                <Button variant="outline" onClick={() => setCreateOpen(true)}>
                  <FontAwesomeIcon icon={["fas", "plus"]} className="text-[11px]" />
                  <span>创建</span>
                </Button>
              </div>
            </div>

            {error && (
              <div className="mb-5 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-[13px] text-red-700">
                {error}
              </div>
            )}

            <div className="mb-6 flex flex-wrap items-center gap-2">
              <div className="relative min-w-[260px] flex-1">
                <div className="flex h-8 items-center rounded-lg bg-ui-tint transition-colors duration-150 ease-out focus-within:bg-ui-tint-strong">
                  <FontAwesomeIcon
                    icon={["fas", "magnifying-glass"]}
                    className="ml-3 shrink-0 text-[12px] text-text-secondary"
                  />
                  <input
                    type="search"
                    value={query}
                    onChange={(event) => setQuery(event.target.value)}
                    placeholder="搜索插件、能力或来源"
                    className="h-full min-w-0 flex-1 border-0 bg-transparent pl-2 pr-3 text-[12px] text-text-base outline-none placeholder:text-text-secondary"
                  />
                </div>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                {originTabs.map((tab) => (
                  <Button
                    key={tab.id}
                    onClick={() => setOrigin(tab.id)}
                    variant={origin === tab.id ? "secondary" : "ghost"}
                    size="sm"
                  >
                    {tab.label}
                  </Button>
                ))}
              </div>
            </div>

            <section className="mb-8">
              <div className="mb-3 flex items-center justify-between border-b border-border-theme pb-2">
                <h2 className="text-[15px] font-semibold text-text-base">已启用 / 已安装</h2>
                <span className="text-[12px] text-text-secondary">{installed.length} 个</span>
              </div>
              {installed.length === 0 ? (
                <EmptyState text="暂无已启用或已安装插件" />
              ) : (
                <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                  {installed.map((plugin) => (
                    <PluginRow
                      key={plugin.id}
                      plugin={plugin}
                      busy={busyId === plugin.id}
                      onOpen={() => setSelectedId(plugin.id)}
                      onToggle={(enabled) => togglePlugin(plugin, enabled)}
                    />
                  ))}
                </div>
              )}
            </section>

            {loading ? (
              <EmptyState text="正在加载插件..." />
            ) : grouped.length === 0 ? (
              <EmptyState text="没有匹配的插件" />
            ) : (
              grouped.map((group) => (
                <section key={group.category} className="mb-8">
                  <div className="mb-3 border-b border-border-theme pb-2">
                    <h2 className="text-[15px] font-semibold text-text-base">
                      {categoryLabel(group.category)}
                    </h2>
                  </div>
                  <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                    {group.plugins.map((plugin) => (
                      <PluginRow
                        key={plugin.id}
                        plugin={plugin}
                        busy={busyId === plugin.id}
                        onOpen={() => setSelectedId(plugin.id)}
                        onToggle={(enabled) => togglePlugin(plugin, enabled)}
                      />
                    ))}
                  </div>
                </section>
              ))
            )}

            <MarketplacePanel
              marketplaces={marketplaces}
              entries={marketplaceEntries}
              busyId={busyId}
              dshBusy={busyId === deepSeekHarnessMarketplaceBusyId}
              onAdd={() => setMarketOpen(true)}
              onAddDeepSeekHarness={submitDeepSeekHarnessMarketplace}
              onRefresh={async (name) => {
                await refreshPluginMarketplace(name);
                await load();
              }}
              onRemove={async (name) => {
                await removePluginMarketplace(name);
                await load();
              }}
              onInstall={installMarketplaceEntry}
            />
          </div>
        </div>
      )}

      {createOpen && (
        <CreateDialog
          draft={createDraft}
          onChange={setCreateDraft}
          onClose={() => setCreateOpen(false)}
          onSubmit={submitCreate}
        />
      )}
      {marketOpen && (
        <MarketplaceDialog
          draft={marketDraft}
          onChange={setMarketDraft}
          onClose={() => setMarketOpen(false)}
          onSubmit={submitMarketplace}
        />
      )}
      {scanDialog && (
        <PluginScanDialog
          action={scanDialog}
          busy={scanBusy}
          onClose={() => {
            if (!scanBusy) setScanDialog(null);
          }}
          onConfirm={confirmScanDialog}
        />
      )}
      {confirmDialog && (
        <ConfirmDialog
          action={confirmDialog}
          busy={confirmBusy}
          onClose={() => {
            if (!confirmBusy) setConfirmDialog(null);
          }}
          onConfirm={confirmActionDialog}
        />
      )}
    </div>
  );
}

function PluginRow({
  plugin,
  busy,
  onOpen,
  onToggle,
}: {
  plugin: Plugin;
  busy: boolean;
  onOpen: () => void;
  onToggle: (enabled: boolean) => void;
}) {
  return (
    <div className="flex min-w-0 items-center gap-3 rounded-lg border border-border-theme px-3 py-3">
      <Button
        type="button"
        onClick={onOpen}
        variant="ghost"
        className="h-auto min-w-0 flex-1 justify-start gap-3 px-0 py-0 text-left hover:bg-transparent"
      >
        <PluginIcon plugin={plugin} />
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <span className="truncate text-[14px] font-semibold text-text-base">
              {plugin.display_name}
            </span>
            <StatusBadge plugin={plugin} />
          </div>
          <div className="mt-1 truncate text-[12px] text-text-secondary">{plugin.description}</div>
          <PluginStateBadges plugin={plugin} />
          <div className="mt-2 flex flex-wrap gap-1">
            <MiniBadge>{originLabel(plugin.origin)}</MiniBadge>
            {plugin.capabilities.slice(0, 3).map((capability) => (
              <MiniBadge key={capability}>{capability}</MiniBadge>
            ))}
          </div>
        </div>
      </Button>
      <ToggleSwitch
        checked={plugin.enabled}
        onChange={() => onToggle(!plugin.enabled)}
        size="sm"
        tone="success"
        disabled={busy || !plugin.available}
        aria-label={`${plugin.enabled ? "禁用" : "启用"} ${plugin.display_name}`}
        title={busy ? "处理中" : plugin.enabled ? "禁用插件" : "启用插件"}
      />
    </div>
  );
}

function PluginDetail({
  plugin,
  outputStyles,
  busy,
  onBack,
  onToggle,
  onUpdate,
  onRemove,
}: {
  plugin: Plugin;
  outputStyles: PluginOutputStyle[];
  busy: boolean;
  onBack: () => void;
  onToggle: (enabled: boolean) => void;
  onUpdate: () => void;
  onRemove: () => void;
}) {
  return (
    <div className="h-full overflow-y-auto custom-scrollbar">
      <div className="mx-auto w-full max-w-[940px] px-8 py-8">
        <Button
          onClick={onBack}
          variant="ghost"
          size="sm"
          className="mb-8 px-0 hover:bg-transparent"
        >
          <FontAwesomeIcon icon={["fas", "chevron-left"]} className="text-[11px]" />
          插件列表
        </Button>

        <div className="mb-8 flex flex-wrap items-start justify-between gap-5">
          <div className="flex min-w-0 items-center gap-4">
            <PluginIcon plugin={plugin} size="lg" />
            <div className="min-w-0">
              <div className="mb-2 flex items-center gap-2">
                <h1 className="truncate text-3xl font-semibold text-text-base">
                  {plugin.display_name}
                </h1>
                <StatusBadge plugin={plugin} />
              </div>
              <p className="text-[14px] text-text-secondary">{plugin.description}</p>
              <PluginStateBadges plugin={plugin} />
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            {plugin.update_available && (
              <Button
                disabled={busy}
                onClick={onUpdate}
                variant="outline"
                className="!bg-elevated-bg !text-text-base hover:!bg-hover-bg"
              >
                <FontAwesomeIcon icon={["fas", "cloud-arrow-down"]} className="text-[12px]" />
                <span>更新</span>
              </Button>
            )}
            <Button
              disabled={busy || !plugin.available}
              onClick={() => onToggle(!plugin.enabled)}
              variant="outline"
              className="!bg-elevated-bg !text-text-base hover:!bg-hover-bg"
            >
              <FontAwesomeIcon
                icon={plugin.enabled ? ["fas", "pause"] : ["fas", "play"]}
                className="text-[12px]"
              />
              <span>{plugin.enabled ? "禁用" : "启用"}</span>
            </Button>
            <Button
              onClick={onRemove}
              variant="outline"
              className="text-red-600 hover:border-red-200 hover:bg-red-50"
            >
              <FontAwesomeIcon
                icon={["fas", plugin.origin === "builtin" ? "pause" : "trash"]}
                className="text-[12px]"
              />
              <span>
                {plugin.origin === "builtin" || plugin.origin === "workspace" ? "禁用" : "卸载"}
              </span>
            </Button>
          </div>
        </div>

        <p className="mb-8 max-w-[760px] text-[14px] leading-7 text-text-secondary">
          {plugin.long_description || plugin.description}
        </p>

        <div className="mb-8 grid grid-cols-2 gap-3 md:grid-cols-5">
          <Metric label="Skills" value={plugin.skill_count} />
          <Metric label="MCP" value={plugin.mcp_server_count} />
          <Metric label="Hooks" value={plugin.hook_count} />
          <Metric label="Commands" value={plugin.command_count} />
          <Metric label="Apps" value={plugin.app_count} />
          <Metric label="Styles" value={plugin.output_style_count ?? 0} />
        </div>

        <InfoSection plugin={plugin} />
        {outputStyles.length > 0 && (
          <OutputStylesSection styles={outputStyles} />
        )}
        {plugin.required_by.length > 0 && <DependentsSection plugin={plugin} />}
        {plugin.errors.length > 0 && <ErrorSection plugin={plugin} />}
      </div>
    </div>
  );
}

function InfoSection({ plugin }: { plugin: Plugin }) {
  return (
    <section className="mb-8">
      <h2 className="mb-4 text-[16px] font-semibold text-text-base">信息</h2>
      <div className="grid grid-cols-[150px_1fr] gap-x-8 gap-y-4 text-[13px]">
        <Info label="ID" value={plugin.id} mono />
        <Info label="来源" value={originLabel(plugin.origin)} />
        <Info label="方言" value={plugin.dialect || "-"} />
        <Info label="开发者" value={plugin.developer || "-"} />
        <Info label="版本" value={plugin.version || "-"} />
        <Info label="分类" value={categoryLabel(plugin.category || "Other")} />
        <Info label="生命周期" value={pluginStateLabel(plugin.state)} />
        <Info label="健康状态" value={pluginHealthLabel(plugin.health_status)} />
        <Info label="执行类型" value={pluginExecutionLabel(plugin.execution_kind)} />
        <Info label="许可证" value={pluginLicenseLabel(plugin.license_status)} />
        <Info label="运行时要求" value={plugin.runtime_required ? "需要" : "不需要"} />
        <Info label="运行时可用" value={plugin.runtime_available ? "可用" : "不可用"} />
        <Info label="入口" value={plugin.entrypoints.join(", ") || "-"} />
        <Info label="运行时载荷" value={plugin.has_runtime_payload ? "有" : "无"} />
        <Info label="最近健康检查" value={plugin.last_health_check || "-"} />
        <Info label="健康错误" value={plugin.health_error || "-"} />
        <Info label="能力" value={plugin.capabilities.join(", ") || "-"} />
        <Info label="权限" value={plugin.permissions.join(", ") || "-"} />
        <Info label="路径" value={plugin.path || "-"} mono />
        <Info label="数据目录" value={plugin.data_dir || "-"} mono />
        <Info label="Manifest" value={plugin.manifest_path || "-"} mono />
        {plugin.overridden_by && <Info label="被覆盖" value={plugin.overridden_by} mono />}
      </div>
    </section>
  );
}

// Agent Plugins §11.3 separates findings that make a plugin unusable from those
// that only skip one component. Rendering both identically overstates the
// second, so severity drives the styling and the section title.
const severityStyles: Record<
  PluginDiagnosticSeverity,
  { container: string; label: string }
> = {
  error: {
    container: "border-red-200 bg-red-50 text-red-800",
    label: "错误",
  },
  warning: {
    container: "border-amber-200 bg-amber-50 text-amber-800",
    label: "已跳过",
  },
  info: {
    container: "border-border-theme bg-ui-tint text-text-secondary",
    label: "提示",
  },
};

/** Missing severity means the payload predates the field; assume the worst. */
function severityOf(error: Plugin["errors"][number]): PluginDiagnosticSeverity {
  return error.severity ?? "error";
}

function ErrorSection({ plugin }: { plugin: Plugin }) {
  const order: PluginDiagnosticSeverity[] = ["error", "warning", "info"];
  const sorted = [...plugin.errors].sort(
    (a, b) => order.indexOf(severityOf(a)) - order.indexOf(severityOf(b)),
  );
  const hasError = sorted.some((error) => severityOf(error) === "error");

  return (
    <section>
      <h2 className="mb-4 text-[16px] font-semibold text-text-base">
        {hasError ? "加载错误" : "加载诊断"}
      </h2>
      <div className="space-y-2">
        {sorted.map((error, index) => {
          const severity = severityOf(error);
          const styles = severityStyles[severity];
          return (
            <div
              key={`${error.kind}-${index}`}
              className={`rounded-lg border px-3 py-2 text-[12px] ${styles.container}`}
            >
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium">{error.kind}</span>
                <MiniBadge>{styles.label}</MiniBadge>
                {error.component && <MiniBadge>{error.component}</MiniBadge>}
              </div>
              <div className="mt-1">{error.message}</div>
              {error.path && <div className="mt-1 font-mono text-[11px]">{error.path}</div>}
            </div>
          );
        })}
      </div>
    </section>
  );
}

function DependentsSection({ plugin }: { plugin: Plugin }) {
  return (
    <section className="mb-8">
      <h2 className="mb-4 text-[16px] font-semibold text-text-base">受影响的插件</h2>
      <div className="space-y-2">
        {plugin.required_by.map((dependent) => (
          <div
            key={dependent.id}
            className="flex items-center justify-between gap-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-3 text-[13px]"
          >
            <div className="min-w-0">
              <div className="truncate font-medium text-text-base">{dependent.display_name}</div>
              <div className="truncate text-[12px] text-text-secondary">{dependent.id}</div>
            </div>
            <MiniBadge tone="warn">依赖此插件</MiniBadge>
          </div>
        ))}
      </div>
    </section>
  );
}

function OutputStylesSection({ styles }: { styles: PluginOutputStyle[] }) {
  return (
    <section className="mb-8">
      <h2 className="mb-4 text-[16px] font-semibold text-text-base">Output Styles</h2>
      <div className="space-y-3">
        {styles.map((style) => (
          <div
            key={style.name}
            className="rounded-lg border border-border-theme bg-white px-4 py-3"
          >
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <div className="truncate text-[14px] font-medium text-text-base">
                    {style.name}
                  </div>
                  {typeof style.force_for_plugin === "boolean" && (
                    <MiniBadge tone={style.force_for_plugin ? "ok" : "neutral"}>
                      {style.force_for_plugin ? "Forced" : "Optional"}
                    </MiniBadge>
                  )}
                </div>
                <div className="mt-1 text-[12px] leading-5 text-text-secondary">
                  {style.description}
                </div>
              </div>
              {style.source_path && (
                <div className="shrink-0 font-mono text-[11px] text-text-tertiary">
                  {style.source_path}
                </div>
              )}
            </div>
            <details className="mt-3">
              <summary className="cursor-pointer text-[12px] text-primary">
                Preview prompt
              </summary>
              <pre className="mt-2 whitespace-pre-wrap rounded-md bg-gray-50 px-3 py-2 text-[12px] leading-6 text-text-base">
                {style.prompt}
              </pre>
            </details>
          </div>
        ))}
      </div>
    </section>
  );
}

function ConfirmDialog({
  action,
  busy,
  onClose,
  onConfirm,
}: {
  action: PendingConfirmAction;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const blocked = Boolean(action.dependents?.length);
  const toneClass =
    action.tone === "danger"
      ? "border-red-200 bg-red-50 text-red-700"
      : "border-amber-200 bg-amber-50 text-amber-800";

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent className="max-w-[640px]">
        <DialogHeader>
          <div>
            <DialogTitle>{action.title}</DialogTitle>
            <DialogDescription>{action.message}</DialogDescription>
          </div>
          <Button variant="ghost" size="icon" disabled={busy} onClick={onClose}>
            <FontAwesomeIcon icon={["fas", "xmark"]} />
          </Button>
        </DialogHeader>

        <div className="min-h-0 flex-1 overflow-y-auto custom-scrollbar px-6 py-5">
          <div className={`mb-5 rounded-lg border px-4 py-3 ${toneClass}`}>
            <div className="text-[13px] font-medium">
              {blocked
                ? "此操作会影响其他已启用插件。"
                : action.tone === "danger"
                  ? "此操作无法撤销。"
                  : "请确认继续。"}
            </div>
          </div>

          {blocked && action.dependents && action.dependents.length > 0 && (
            <section>
              <h3 className="mb-3 text-[14px] font-semibold text-text-base">受影响的插件</h3>
              <div className="space-y-2">
                {action.dependents.map((dependent) => (
                  <div
                    key={dependent.id}
                    className="rounded-lg border border-border-theme px-3 py-2 text-[12px]"
                  >
                    <div className="font-medium text-text-base">{dependent.display_name}</div>
                    <div className="mt-1 break-words text-text-secondary">{dependent.id}</div>
                  </div>
                ))}
              </div>
            </section>
          )}
        </div>

        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={onClose}>
            取消
          </Button>
          <Button
            variant={action.tone === "danger" ? "destructive" : "default"}
            disabled={busy}
            onClick={onConfirm}
          >
            {busy ? "处理中..." : action.submitLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function MarketplacePanel({
  marketplaces,
  entries,
  busyId,
  dshBusy,
  onAdd,
  onAddDeepSeekHarness,
  onRefresh,
  onRemove,
  onInstall,
}: {
  marketplaces: PluginMarketplace[];
  entries: PluginMarketplaceEntry[];
  busyId: string | null;
  dshBusy: boolean;
  onAdd: () => void;
  onAddDeepSeekHarness: () => Promise<void>;
  onRefresh: (name: string) => Promise<void>;
  onRemove: (name: string) => Promise<void>;
  onInstall: (entry: PluginMarketplaceEntry) => Promise<void>;
}) {
  const dshMarketplace = marketplaces.find(isDeepSeekHarnessMarketplace);
  const entriesByMarketplace = new Map<string, PluginMarketplaceEntry[]>();
  for (const entry of entries) {
    const list = entriesByMarketplace.get(entry.marketplace) ?? [];
    list.push(entry);
    entriesByMarketplace.set(entry.marketplace, list);
  }
  const marketplaceNames = [
    ...marketplaces.map((marketplace) => marketplace.name),
    ...entries
      .map((entry) => entry.marketplace)
      .filter((name) => !marketplaces.some((marketplace) => marketplace.name === name)),
  ];

  return (
    <section className="border-t border-border-theme pt-5">
      <div className="mb-3 flex items-center justify-between">
        <div>
          <h2 className="text-[15px] font-semibold text-text-base">
            DeepSeek Harness 插件市场
          </h2>
          <div className="mt-1 text-[12px] text-text-secondary">
            从 GitHub `dsh-plugin` topic 获取市场源和可安装插件。
          </div>
        </div>
        <div className="flex flex-wrap justify-end gap-2">
          <Button
            variant={dshMarketplace ? "secondary" : "default"}
            size="sm"
            disabled={dshBusy}
            onClick={() => void onAddDeepSeekHarness()}
            title={dshMarketplace ? "刷新 DeepSeek Harness 市场" : "接入 DeepSeek Harness 市场"}
          >
            <FontAwesomeIcon icon={["fas", dshMarketplace ? "rotate" : "plug"]} />
            <span>{dshBusy ? "处理中..." : dshMarketplace ? "刷新 DSH" : "接入 DSH"}</span>
          </Button>
          <Button variant="outline" size="sm" onClick={onAdd}>
            <FontAwesomeIcon icon={["fas", "plus"]} />
            <span>添加来源</span>
          </Button>
        </div>
      </div>
      {marketplaceNames.length === 0 ? (
        <EmptyState text="尚未接入插件市场，点击“接入 DSH”后会拉取可安装插件列表" />
      ) : (
        <div className="space-y-4">
          {marketplaceNames.map((name) => {
            const marketplace = marketplaces.find((item) => item.name === name);
            const marketplaceEntries = entriesByMarketplace.get(name) ?? [];
            return (
              <div key={name} className="rounded-lg border border-border-theme px-3 py-3">
                <div className="flex items-center justify-between gap-3">
                  <div className="min-w-0">
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="truncate font-medium text-text-base">{name}</span>
                      {marketplace && isDeepSeekHarnessMarketplace(marketplace) && (
                        <MiniBadge tone="info">DSH</MiniBadge>
                      )}
                      <MiniBadge tone="neutral">{marketplaceEntries.length} 个插件</MiniBadge>
                    </div>
                    <div className="mt-1 truncate text-[12px] text-text-secondary">
                      {marketplace?.source || "已发现市场插件"}
                    </div>
                    {marketplace?.last_updated && (
                      <div className="mt-1 text-[11px] text-text-tertiary">
                        最近刷新：{marketplace.last_updated}
                      </div>
                    )}
                  </div>
                  {marketplace && (
                    <div className="flex gap-2">
                      <Button
                        variant="ghost"
                        size="icon"
                        onClick={() => void onRefresh(marketplace.name)}
                        title="刷新"
                      >
                        <FontAwesomeIcon icon={["fas", "rotate"]} />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        onClick={() => void onRemove(marketplace.name)}
                        title="删除"
                      >
                        <FontAwesomeIcon icon={["fas", "trash"]} />
                      </Button>
                    </div>
                  )}
                </div>
                {marketplaceEntries.length === 0 ? (
                  <div className="mt-3">
                    <EmptyState text="暂无可安装插件，刷新市场后显示插件列表" />
                  </div>
                ) : (
                  <div className="mt-3 grid grid-cols-1 gap-2 md:grid-cols-2">
                    {marketplaceEntries.map((entry) => (
                      <MarketplaceEntryCard
                        key={`${entry.marketplace}:${entry.name}`}
                        entry={entry}
                        busy={busyId === `${entry.marketplace}:${entry.name}`}
                        onInstall={onInstall}
                      />
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

function MarketplaceEntryCard({
  entry,
  busy,
  onInstall,
}: {
  entry: PluginMarketplaceEntry;
  busy: boolean;
  onInstall: (entry: PluginMarketplaceEntry) => Promise<void>;
}) {
  const installHint = marketplaceInstallHint(entry);
  return (
    <div className="flex min-w-0 items-center justify-between gap-3 rounded-lg bg-bg-subtle px-3 py-3 text-[13px]">
      <div className="min-w-0">
        <div className="font-medium text-text-base">{entry.display_name}</div>
        <div className="mt-1 line-clamp-2 text-[12px] text-text-secondary">
          {entry.description}
        </div>
        <div className="mt-2 flex flex-wrap gap-2 text-[11px] text-text-tertiary">
          <span>{entry.source_kind}</span>
          {entry.version && <span>{entry.version}</span>}
          {entry.license && <span>{entry.license}</span>}
          {entry.source_commit && <span>{entry.source_commit.slice(0, 12)}</span>}
          {entry.policy_installation && (
            <MiniBadge tone={entry.installable ? "neutral" : "warn"}>
              {entry.policy_installation}
            </MiniBadge>
          )}
          {entry.policy_authentication && (
            <MiniBadge tone={entry.authentication_required ? "warn" : "neutral"}>
              {entry.policy_authentication}
            </MiniBadge>
          )}
          {entry.runtime_required && <MiniBadge tone="warn">需要运行时</MiniBadge>}
          {entry.has_runtime_payload && <MiniBadge tone="info">含运行时包</MiniBadge>}
        </div>
        {!entry.installable && installHint && (
          <div className="mt-2 max-w-[420px] text-[12px] leading-5 text-amber-700">
            {installHint}
          </div>
        )}
      </div>
      <Button
        variant={entry.installed && !entry.update_available ? "outline" : "default"}
        disabled={busy || !entry.installable}
        onClick={() => void onInstall(entry)}
        title={installHint || (entry.installable ? "安装插件" : "该来源不可安装")}
      >
        {busy
          ? "处理中..."
          : entry.update_available
            ? "更新"
            : entry.installed
              ? "重新安装"
              : "安装"}
      </Button>
    </div>
  );
}

function isDeepSeekHarnessMarketplace(marketplace: PluginMarketplace): boolean {
  return (
    marketplace.name === deepSeekHarnessMarketplaceDraft.name ||
    normalizeMarketplaceSource(marketplace.source) ===
      normalizeMarketplaceSource(deepSeekHarnessMarketplaceDraft.source)
  );
}

function normalizeMarketplaceSource(source: string): string {
  return source.trim().replace(/\/+$/, "").toLowerCase();
}

function CreateDialog({
  draft,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: CreatePluginDraft;
  onChange: (draft: CreatePluginDraft) => void;
  onClose: () => void;
  onSubmit: () => void;
}) {
  return (
    <Modal
      title="创建插件"
      onClose={onClose}
      onSubmit={onSubmit}
      submitLabel="创建"
      disabled={!draft.name?.trim()}
    >
      <Field label="名称">
        <Input
          value={draft.name || ""}
          onChange={(event) => onChange({ ...draft, name: event.target.value })}
          placeholder="office-helper"
        />
      </Field>
      <Field label="说明">
        <Textarea
          value={draft.description || ""}
          onChange={(event) => onChange({ ...draft, description: event.target.value })}
          placeholder="这个插件提供哪些技能、命令、MCP 或 hooks"
        />
      </Field>
      <Field label="目录">
        <Input
          value={draft.directory || ""}
          onChange={(event) => onChange({ ...draft, directory: event.target.value })}
          placeholder="office-helper"
        />
      </Field>
      <Field label="分类">
        <Input
          value={draft.category || ""}
          onChange={(event) => onChange({ ...draft, category: event.target.value })}
          placeholder="Developer Tools"
        />
      </Field>
    </Modal>
  );
}

function MarketplaceDialog({
  draft,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: AddPluginMarketplaceInput;
  onChange: (draft: AddPluginMarketplaceInput) => void;
  onClose: () => void;
  onSubmit: () => void;
}) {
  return (
    <Modal
      title="添加插件市场"
      onClose={onClose}
      onSubmit={onSubmit}
      submitLabel="添加"
      disabled={!draft.source.trim()}
    >
      <Field label="名称">
        <Input
          value={draft.name || ""}
          onChange={(event) => onChange({ ...draft, name: event.target.value })}
          placeholder="openai-curated"
        />
      </Field>
      <Field label="来源">
        <Input
          value={draft.source}
          onChange={(event) => onChange({ ...draft, source: event.target.value })}
          placeholder="G:/plugins 或 https://github.com/org/plugins.git"
        />
      </Field>
      <Field label="Git 引用">
        <Input
          value={draft.git_ref || ""}
          onChange={(event) => onChange({ ...draft, git_ref: event.target.value })}
          placeholder="main"
        />
      </Field>
      <Field label="子路径">
        <Input
          value={draft.sparse_path || ""}
          onChange={(event) => onChange({ ...draft, sparse_path: event.target.value })}
          placeholder="plugins/browser"
        />
      </Field>
    </Modal>
  );
}

function PluginScanDialog({
  action,
  busy,
  onClose,
  onConfirm,
}: {
  action: PendingScanAction;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const { report } = action;
  const blocked = report.errors.length > 0;
  const highCount = highRiskCount(report);

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent className="max-w-[760px]">
        <DialogHeader>
          <div>
            <DialogTitle>{action.title}</DialogTitle>
            <DialogDescription>
              安装前扫描会检查 manifest、权限声明、MCP、hooks 和文件风险。
            </DialogDescription>
          </div>
          <Button variant="ghost" size="icon" disabled={busy} onClick={onClose}>
            <FontAwesomeIcon icon={["fas", "xmark"]} />
          </Button>
        </DialogHeader>

        <div className="min-h-0 flex-1 overflow-y-auto custom-scrollbar px-6 py-5">
          <div
            className={`mb-5 rounded-lg border px-4 py-3 ${
              blocked
                ? "border-red-200 bg-red-50 text-red-700"
                : highCount > 0
                  ? "border-amber-200 bg-amber-50 text-amber-800"
                  : "border-emerald-200 bg-emerald-50 text-emerald-700"
            }`}
          >
            <div className="flex items-center gap-2 text-[13px] font-medium">
              <FontAwesomeIcon
                icon={
                  blocked
                    ? ["fas", "circle-exclamation"]
                    : highCount > 0
                      ? ["fas", "triangle-exclamation"]
                      : ["fas", "circle-check"]
                }
              />
              {blocked
                ? "扫描发现阻断错误，无法安装。"
                : highCount > 0
                  ? `扫描发现 ${highCount} 个高风险项，需要确认后继续。`
                  : "扫描完成，未发现高风险项。"}
            </div>
          </div>

          {action.authenticationHint && !blocked && (
            <div className="mb-5 rounded-lg border border-sky-200 bg-sky-50 px-4 py-3 text-[13px] leading-5 text-sky-800">
              <div className="mb-1 flex items-center gap-2 font-medium">
                <FontAwesomeIcon icon={["fas", "key"]} />
                认证策略
              </div>
              <div>{action.authenticationHint}</div>
            </div>
          )}

          <div className="mb-5 grid grid-cols-2 gap-3 md:grid-cols-4">
            <ScanMetric label="Manifest" value={report.manifest_ok ? "有效" : "无效"} />
            <ScanMetric label="插件" value={report.plugin_name || "-"} />
            <ScanMetric label="文件" value={String(report.file_count)} />
            <ScanMetric label="体积" value={formatBytes(report.total_bytes)} />
          </div>

          {report.component_summaries.length > 0 && (
            <section className="mb-5">
              <div className="mb-3 flex items-center justify-between">
                <h3 className="text-[14px] font-semibold text-text-base">能力摘要</h3>
                <span className="text-[12px] text-text-secondary">
                  {report.component_summaries.length} 项
                </span>
              </div>
              <div className="space-y-2">
                {report.component_summaries.map((summary, index) => (
                  <div
                    key={`${summary.kind}-${summary.name}-${index}`}
                    className="rounded-lg border border-border-theme px-3 py-3 text-[12px]"
                  >
                    <div className="mb-1 flex flex-wrap items-center gap-2">
                      <MiniBadge tone="neutral">{componentKindLabel(summary.kind)}</MiniBadge>
                      <span className="font-medium text-text-base">{summary.name}</span>
                    </div>
                    <div className="leading-5 text-text-secondary">{summary.description}</div>
                    {summary.details.length > 0 && (
                      <div className="mt-2 flex flex-wrap gap-2">
                        {summary.details.map((detail) => (
                          <MiniBadge key={detail} tone="neutral">
                            {detail}
                          </MiniBadge>
                        ))}
                      </div>
                    )}
                    {summary.path && (
                      <div className="mt-2 break-words font-mono text-[11px] text-text-tertiary">
                        {summary.path}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            </section>
          )}

          <div className="mb-5 grid grid-cols-[110px_1fr] gap-x-5 gap-y-3 text-[13px]">
            <Info label="来源" value={report.source_dir} mono />
          </div>

          <section className="mb-5">
            <div className="mb-3 flex items-center justify-between">
              <h3 className="text-[14px] font-semibold text-text-base">风险项</h3>
              <span className="text-[12px] text-text-secondary">{report.risks.length} 项</span>
            </div>
            {report.risks.length === 0 ? (
              <EmptyState text="没有发现风险项" />
            ) : (
              <div className="space-y-2">
                {sortedRisks(report.risks).map((risk, index) => (
                  <div
                    key={`${risk.severity}-${risk.category}-${risk.title}-${index}`}
                    className="rounded-lg border border-border-theme px-3 py-3 text-[12px]"
                  >
                    <div className="mb-1 flex flex-wrap items-center gap-2">
                      <SeverityBadge severity={risk.severity} />
                      <span className="font-medium text-text-base">{risk.title}</span>
                      <span className="text-text-tertiary">{risk.category}</span>
                    </div>
                    <div className="leading-5 text-text-secondary">{risk.detail}</div>
                    {risk.path && (
                      <div className="mt-2 break-words font-mono text-[11px] text-text-tertiary">
                        {risk.path}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}
          </section>

          {report.errors.length > 0 && (
            <section>
              <h3 className="mb-3 text-[14px] font-semibold text-text-base">阻断错误</h3>
              <div className="space-y-2">
                {report.errors.map((item, index) => (
                  <div
                    key={`${item}-${index}`}
                    className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-[12px] text-red-700"
                  >
                    {item}
                  </div>
                ))}
              </div>
            </section>
          )}
        </div>

        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={onClose}>
            {blocked ? "关闭" : "取消"}
          </Button>
          {!blocked && (
            <Button disabled={busy} onClick={onConfirm}>
              {busy ? "安装中..." : action.submitLabel}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function Modal({
  title,
  children,
  onClose,
  onSubmit,
  submitLabel,
  disabled,
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
  onSubmit: () => void;
  submitLabel: string;
  disabled?: boolean;
}) {
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <Button variant="ghost" size="icon" onClick={onClose}>
            <FontAwesomeIcon icon={["fas", "xmark"]} />
          </Button>
        </DialogHeader>
        <div className="space-y-4 px-6 py-5">{children}</div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            取消
          </Button>
          <Button
            disabled={disabled}
            onClick={onSubmit}
            variant="outline"
            className="!bg-elevated-bg !text-text-base hover:!bg-hover-bg"
          >
            {submitLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ScanMetric({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 rounded-lg border border-border-theme px-3 py-3">
      <div className="text-[11px] uppercase tracking-wide text-text-secondary">{label}</div>
      <div className="mt-1 truncate text-[13px] font-semibold text-text-base">{value}</div>
    </div>
  );
}

function PluginIcon({ plugin, size = "md" }: { plugin: Plugin; size?: "md" | "lg" }) {
  const icon = iconForPlugin(plugin);
  const large = size === "lg";
  const color = plugin.brand_color || "#334155";
  return (
    <div
      className={`${large ? "h-[30px] w-[30px] text-[13px]" : "h-[22px] w-[22px] text-[10px]"} flex shrink-0 items-center justify-center rounded-md text-white`}
      style={{ backgroundColor: color }}
    >
      <FontAwesomeIcon icon={icon} />
    </div>
  );
}

function iconForPlugin(plugin: Plugin): IconProp {
  const name = `${plugin.name} ${plugin.display_name}`.toLowerCase();
  if (name.includes("browser") || name.includes("chrome")) return ["fas", "arrow-pointer"];
  if (name.includes("computer")) return ["fas", "desktop"];
  if (name.includes("office") || name.includes("doc")) return ["far", "file-lines"];
  if (name.includes("meeting") || name.includes("record")) return ["fas", "microphone"];
  if (plugin.hook_count > 0) return ["fas", "anchor"];
  if (plugin.mcp_server_count > 0) return ["fas", "plug"];
  if (plugin.skill_count > 0) return ["fas", "wand-magic-sparkles"];
  if ((plugin.output_style_count ?? 0) > 0) return ["fas", "pen"];
  return ["fas", "puzzle-piece"];
}

function StatusBadge({ plugin }: { plugin: Plugin }) {
  if (!plugin.available) return <MiniBadge tone="warn">不可用</MiniBadge>;
  if (plugin.enabled) return <MiniBadge tone="ok">已启用</MiniBadge>;
  return <MiniBadge>已禁用</MiniBadge>;
}

function MiniBadge({
  children,
  tone = "neutral",
}: {
  children: ReactNode;
  tone?: "neutral" | "ok" | "warn" | "info" | "danger";
}) {
  let cls = "bg-gray-100 text-text-secondary";
  switch (tone) {
    case "ok":
      cls = "bg-emerald-50 text-emerald-700";
      break;
    case "warn":
      cls = "bg-amber-50 text-amber-700";
      break;
    case "info":
      cls = "bg-sky-50 text-sky-700";
      break;
    case "danger":
      cls = "bg-red-50 text-red-700";
      break;
    default:
      break;
  }
  return <span className={`rounded-full px-2 py-0.5 text-[11px] ${cls}`}>{children}</span>;
}

function PluginStateBadges({ plugin }: { plugin: Plugin }) {
  return (
    <div className="mt-2 flex flex-wrap gap-1.5">
      <MiniBadge tone={pluginStateTone(plugin.state)}>{pluginStateLabel(plugin.state)}</MiniBadge>
      <MiniBadge tone={pluginHealthTone(plugin.health_status)}>
        {pluginHealthLabel(plugin.health_status)}
      </MiniBadge>
      <MiniBadge tone={pluginExecutionTone(plugin.execution_kind)}>
        {pluginExecutionLabel(plugin.execution_kind)}
      </MiniBadge>
      <MiniBadge tone={pluginLicenseTone(plugin.license_status)}>
        {pluginLicenseLabel(plugin.license_status)}
      </MiniBadge>
      {plugin.runtime_required && (
        <MiniBadge tone={plugin.runtime_available ? "ok" : "warn"}>
          {plugin.runtime_available ? "运行时就绪" : "需要运行时"}
        </MiniBadge>
      )}
      {plugin.has_runtime_payload && <MiniBadge tone="info">含运行时包</MiniBadge>}
    </div>
  );
}

function Metric({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-lg border border-border-theme px-3 py-3">
      <div className="text-[11px] uppercase tracking-wide text-text-secondary">{label}</div>
      <div className="mt-1 text-xl font-semibold text-text-base">{value}</div>
    </div>
  );
}

function Info({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <>
      <div className="text-text-secondary">{label}</div>
      <div className={`${mono ? "font-mono text-[12px]" : ""} min-w-0 break-words text-text-base`}>
        {value}
      </div>
    </>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Label className="block">
      <div className="mb-1.5">{label}</div>
      {children}
    </Label>
  );
}

function EmptyState({ text }: { text: string }) {
  return (
    <div className="rounded-lg border border-dashed border-border-theme px-4 py-8 text-center text-[13px] text-text-secondary">
      {text}
    </div>
  );
}

function SeverityBadge({ severity }: { severity: string }) {
  const normalized = severity.toLowerCase();
  const cls =
    normalized === "critical"
      ? "bg-red-100 text-red-700"
      : normalized === "high"
        ? "bg-amber-100 text-amber-800"
        : normalized === "medium"
          ? "bg-sky-100 text-sky-700"
          : "bg-gray-100 text-text-secondary";
  return (
    <span className={`rounded-full px-2 py-0.5 text-[11px] font-medium ${cls}`}>
      {severityLabel(normalized)}
    </span>
  );
}

function pluginStateLabel(state: Plugin["state"]): string {
  switch (state) {
    case "discovered":
      return "已发现";
    case "parsed":
      return "已解析";
    case "installed":
      return "已安装";
    case "runtime_ready":
      return "运行时就绪";
    case "executable":
      return "可执行";
    case "verified":
      return "已验证";
    case "incomplete":
      return "待补全";
    case "failed":
      return "失败";
    default:
      return state;
  }
}

function pluginHealthLabel(status: Plugin["health_status"]): string {
  switch (status) {
    case "ready":
      return "健康";
    case "needs_configuration":
      return "需要配置";
    case "needs_authorization":
      return "需要授权";
    case "connection_unavailable":
      return "连接不可用";
    case "runtime_unavailable":
      return "运行时不可用";
    case "incomplete":
      return "待补全";
    case "failed":
      return "失败";
    case "unknown":
      return "未知";
    default:
      return status;
  }
}

function pluginExecutionLabel(kind: Plugin["execution_kind"]): string {
  switch (kind) {
    case "host_backed":
      return "宿主适配器";
    case "skill_only":
      return "仅技能";
    case "subprocess":
      return "子进程";
    case "managed_runtime":
      return "托管运行时";
    case "dsh_sidecar":
      return "DSH Sidecar";
    default:
      return kind;
  }
}

function pluginLicenseLabel(status: Plugin["license_status"]): string {
  switch (status) {
    case "first_party":
      return "第一方";
    case "bundled_third_party":
      return "第三方内置";
    case "marketplace_only":
      return "仅市场";
    case "missing":
      return "缺失";
    case "unknown":
      return "未知";
    default:
      return status;
  }
}

function pluginStateTone(state: Plugin["state"]): "neutral" | "ok" | "warn" | "info" | "danger" {
  switch (state) {
    case "verified":
    case "executable":
      return "ok";
    case "runtime_ready":
      return "info";
    case "incomplete":
      return "warn";
    case "failed":
      return "danger";
    default:
      return "neutral";
  }
}

function pluginHealthTone(
  status: Plugin["health_status"],
): "neutral" | "ok" | "warn" | "info" | "danger" {
  switch (status) {
    case "ready":
      return "ok";
    case "needs_configuration":
    case "needs_authorization":
    case "incomplete":
      return "warn";
    case "connection_unavailable":
    case "runtime_unavailable":
      return "info";
    case "failed":
      return "danger";
    case "unknown":
    default:
      return "neutral";
  }
}

function pluginExecutionTone(
  kind: Plugin["execution_kind"],
): "neutral" | "ok" | "warn" | "info" | "danger" {
  switch (kind) {
    case "host_backed":
    case "skill_only":
      return "ok";
    case "subprocess":
      return "info";
    case "managed_runtime":
      return "warn";
    case "dsh_sidecar":
      return "neutral";
    default:
      return "neutral";
  }
}

function pluginLicenseTone(
  status: Plugin["license_status"],
): "neutral" | "ok" | "warn" | "info" | "danger" {
  switch (status) {
    case "first_party":
      return "ok";
    case "bundled_third_party":
      return "info";
    case "marketplace_only":
      return "warn";
    case "missing":
      return "danger";
    case "unknown":
    default:
      return "neutral";
  }
}

function originLabel(origin: string): string {
  switch (origin) {
    case "builtin":
      return "DeepAgent 提供";
    case "workspace":
      return "工作区";
    case "personal":
      return "个人";
    case "marketplace":
      return "市场";
    case "session":
      return "会话";
    default:
      return origin;
  }
}

function highRiskCount(report: PluginScanReport): number {
  return report.risks.filter((risk) =>
    ["high", "critical"].includes(risk.severity.toLowerCase()),
  ).length;
}

function sortedRisks(risks: PluginScanReport["risks"]): PluginScanReport["risks"] {
  const rank: Record<string, number> = {
    critical: 0,
    high: 1,
    medium: 2,
    low: 3,
  };
  return [...risks].sort((a, b) => {
    const left = rank[a.severity.toLowerCase()] ?? 9;
    const right = rank[b.severity.toLowerCase()] ?? 9;
    return left - right || a.category.localeCompare(b.category) || a.title.localeCompare(b.title);
  });
}

function severityLabel(severity: string): string {
  switch (severity) {
    case "critical":
      return "严重";
    case "high":
      return "高";
    case "medium":
      return "中";
    case "low":
      return "低";
    default:
      return severity;
  }
}

function componentKindLabel(kind: string): string {
  switch (kind) {
    case "skill":
      return "技能";
    case "command":
      return "命令";
    case "agent":
      return "Agent";
    case "output-style":
      return "输出风格";
    default:
      return kind;
  }
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(value >= 10 || index === 0 ? 0 : 1)} ${units[index]}`;
}

function categoryLabel(category: string): string {
  switch (category) {
    case "Featured":
      return "精选";
    case "Productivity":
      return "效率";
    case "Developer Tools":
      return "开发工具";
    case "Data & Analytics":
      return "数据与分析";
    case "Creativity":
      return "创意";
    case "Security":
      return "安全";
    case "Other":
      return "其他";
    default:
      return category;
  }
}

function marketplaceInstallHint(entry: PluginMarketplaceEntry): string | null {
  if (entry.install_block_reason) {
    return entry.install_block_reason;
  }
  if (entry.installable) {
    if (entry.authentication_required) {
      return (
        entry.authentication_hint ||
        `安装时需要认证: ${entry.policy_authentication || "ON_INSTALL"}`
      );
    }
    return entry.policy_authentication
      ? `认证策略: ${entry.policy_authentication}`
      : null;
  }
  return entry.source || "该来源不可安装";
}
