import { Download, Ellipsis, FolderOpen, Package, RefreshCw, Search } from "lucide-react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  runtimeCancel,
  runtimeInstall,
  runtimeList,
  runtimeMigrateResources,
  runtimeProgressSubscribe,
  runtimeRoots,
  runtimeUninstall,
} from "../../api";
import type { RuntimeProgress, RuntimeRoots, RuntimeStatus } from "../../types";
import { message } from "../message";
import { Button } from "../shadcn/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "../shadcn/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "../shadcn/dropdown-menu";
import { Input } from "../shadcn/input";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemTitle } from "../shadcn/item";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "../shadcn/tooltip";

const groups = ["运行环境", "语音输入", "多媒体与文档", "其他资源"] as const;
type ResourceGroup = typeof groups[number];

const resourceVisuals: Partial<Record<string, readonly [ResourceGroup, string]>> = {
  "node": ["运行环境", new URL("../../runtime-logos/node.svg", import.meta.url).href],
  "python": ["运行环境", new URL("../../runtime-logos/python.svg", import.meta.url).href],
  "java": ["运行环境", new URL("../../runtime-logos/temurin.png", import.meta.url).href],
  "speech-model": ["语音输入", new URL("../../runtime-logos/whisper.png", import.meta.url).href],
  "speech-engine": ["语音输入", new URL("../../runtime-logos/whisper-cpp.jpg", import.meta.url).href],
  "video-media-tools": ["多媒体与文档", new URL("../../runtime-logos/ffmpeg.ico", import.meta.url).href],
  "doc-convert": ["多媒体与文档", new URL("../../runtime-logos/pandoc.svg", import.meta.url).href],
  "pdf-render": ["多媒体与文档", new URL("../../runtime-logos/chromium.svg", import.meta.url).href],
  "office-suite": ["多媒体与文档", new URL("../../runtime-logos/libreoffice.png", import.meta.url).href],
};

function resourceVisual(runtime: RuntimeStatus): readonly [ResourceGroup, string?] {
  return resourceVisuals[runtime.capability] ?? ["其他资源"];
}

type ActiveDownload = { started: boolean; ready: boolean; cancelRequested: boolean };

function unavailableReason(runtime: RuntimeStatus) {
  if (runtime.installed) return undefined;
  if (!runtime.available_for_platform) return "当前系统不可用";
}

const filters = ["全部", "已安装", "未安装", "下载中"] as const;
type ResourceFilter = typeof filters[number];
const outlineButton = "bg-elevated-bg shadow-none hover:bg-hover-bg";

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  if (!Number.isFinite(bytes) || bytes < 0) return "未知大小";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

function runtimeProgressPercent(progress?: RuntimeProgress): number | null {
  if (!progress?.total || progress.total <= 0) return null;
  return Math.min(100, Math.round((progress.downloaded / progress.total) * 100));
}

export function RuntimeResourceSettings() {
  const { t } = useTranslation();
  const [runtimes, setRuntimes] = useState<RuntimeStatus[]>([]);
  const [roots, setRoots] = useState<RuntimeRoots | null>(null);
  const [loading, setLoading] = useState(true);
  const [installingIds, setInstallingIds] = useState<Set<string>>(new Set());
  const activeDownloads = useRef(new Map<string, ActiveDownload>());
  const [cancellingIds, setCancellingIds] = useState<Set<string>>(new Set());
  const [committingIds, setCommittingIds] = useState<Set<string>>(new Set());
  const [failedIds, setFailedIds] = useState<Set<string>>(new Set());
  const [uninstallTarget, setUninstallTarget] = useState<RuntimeStatus | null>(null);
  const [uninstalling, setUninstalling] = useState(false);
  const [filter, setFilter] = useState<ResourceFilter>("全部");
  const [query, setQuery] = useState("");
  const [progress, setProgress] = useState<Record<string, RuntimeProgress>>({});

  const refresh = () => {
    setLoading(true);
    Promise.all([runtimeList(), runtimeRoots()])
      .then(([items, nextRoots]) => {
        setRuntimes(items);
        setRoots(nextRoots);
      })
      .catch((e) => {
        console.error("runtime resources failed:", e);
        message.error("资源列表加载失败");
      })
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    refresh();
  }, []);

  const interruptDownload = async (runtime: RuntimeStatus, transfer: ActiveDownload) => {
    try {
      const cancelled = await runtimeCancel(runtime.id);
      if (!cancelled && activeDownloads.current.get(runtime.id) === transfer) {
        setCommittingIds((ids) => new Set(ids).add(runtime.id));
      }
    } catch (error) {
      console.error("runtime_cancel failed:", error);
      if (activeDownloads.current.get(runtime.id) === transfer) {
        transfer.cancelRequested = false;
        message.error(`${runtime.name} 取消失败`);
      }
    } finally {
      if (activeDownloads.current.get(runtime.id) === transfer) {
        setCancellingIds((ids) => new Set([...ids].filter((id) => id !== runtime.id)));
      }
    }
  };

  const installRuntime = async (runtime: RuntimeStatus) => {
    if (activeDownloads.current.has(runtime.id)) return;
    const transfer = { started: false, ready: false, cancelRequested: false };
    activeDownloads.current.set(runtime.id, transfer);
    setInstallingIds((ids) => new Set(ids).add(runtime.id));
    setFailedIds((ids) => new Set([...ids].filter((id) => id !== runtime.id)));
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await runtimeProgressSubscribe(runtime.id, (p) => {
        if (!transfer.started) return;
        if (!transfer.ready) {
          transfer.ready = true;
          if (transfer.cancelRequested) void interruptDownload(runtime, transfer);
        }
        setProgress((prev) => ({ ...prev, [runtime.id]: p }));
      });
      if (transfer.cancelRequested) {
        message.info(`${runtime.name} 已取消下载`);
        return;
      }
      transfer.started = true;
      const installed = await runtimeInstall(runtime.id);
      setRuntimes((items) => items.map((item) => item.id === installed.id ? installed : item));
      message.success(`${runtime.name} 已安装`);
      refresh();
    } catch (error) {
      if (String(error) === "download cancelled") {
        message.info(`${runtime.name} 已取消下载`);
      } else {
        console.error("runtime_install failed:", error);
        setFailedIds((ids) => new Set(ids).add(runtime.id));
        message.error(`${runtime.name} 安装失败`);
      }
    } finally {
      unlisten?.();
      activeDownloads.current.delete(runtime.id);
      setCancellingIds((ids) => new Set([...ids].filter((id) => id !== runtime.id)));
      setCommittingIds((ids) => new Set([...ids].filter((id) => id !== runtime.id)));
      setInstallingIds((ids) => new Set([...ids].filter((id) => id !== runtime.id)));
      setProgress((prev) => {
        const next = { ...prev };
        delete next[runtime.id];
        return next;
      });
    }
  };

  const uninstallRuntime = async (runtime: RuntimeStatus) => {
    setUninstalling(true);
    try {
      if (await runtimeUninstall(runtime.id)) {
        message.success(`${runtime.name} 已卸载`);
      } else {
        message.info(`${runtime.name} 没有可卸载的安装`);
      }
      setUninstallTarget(null);
      refresh();
    } catch (error) {
      console.error("runtime_uninstall failed:", error);
      message.error(`${runtime.name} 卸载失败`);
    } finally {
      setUninstalling(false);
    }
  };

  const revealRuntime = async (runtime: RuntimeStatus) => {
    if (!runtime.install_path) return;
    try {
      await revealItemInDir(runtime.install_path);
    } catch (error) {
      console.error("reveal runtime failed:", error);
      message.error(`无法定位 ${runtime.name}`);
    }
  };

  const cancelRuntime = (runtime: RuntimeStatus) => {
    const transfer = activeDownloads.current.get(runtime.id);
    if (!transfer || transfer.cancelRequested) return;
    transfer.cancelRequested = true;
    setCancellingIds((ids) => new Set(ids).add(runtime.id));
    if (transfer.ready) void interruptDownload(runtime, transfer);
  };

  const migrateResources = async () => {
    setLoading(true);
    try {
      const items = await runtimeMigrateResources();
      setRuntimes(items);
      setRoots(await runtimeRoots());
      message.success("资源已迁移到当前资源目录");
    } catch (e) {
      console.error("runtime_migrate_resources failed:", e);
      message.error("资源迁移失败，旧目录仍会继续作为兼容读取目录");
    } finally {
      setLoading(false);
    }
  };

  const byFilter: Record<ResourceFilter, RuntimeStatus[]> = {
    全部: runtimes,
    已安装: runtimes.filter((runtime) => runtime.installed),
    未安装: runtimes.filter((runtime) => !runtime.installed && !installingIds.has(runtime.id)),
    下载中: runtimes.filter((runtime) => installingIds.has(runtime.id)),
  };
  const installedSize = byFilter.已安装.reduce((bytes, runtime) => bytes + runtime.size_bytes, 0);
  const needle = query.trim().toLowerCase();
  const visibleRuntimes = byFilter[filter].filter((runtime) =>
    [runtime.name, runtime.id, runtime.version, resourceVisual(runtime)[0]].some((value) => value.toLowerCase().includes(needle)),
  );

  return (
    <TooltipProvider>
      <div className="mb-12 space-y-5">
        <header className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="text-xl font-semibold text-text-base">{t("settings.tabs.resources")}</h2>
          <div className="flex flex-wrap items-center gap-2">
            {!!roots?.fallback_roots.length && (
              <Button variant="outline" size="sm" className={outlineButton} onClick={migrateResources} disabled={loading || installingIds.size > 0 || uninstalling}>
                <Download aria-hidden="true" className="size-3.5" />
                迁移资源
              </Button>
            )}
            <Button variant="outline" size="sm" className={outlineButton} onClick={refresh} disabled={loading}>
              <RefreshCw aria-hidden="true" className={`size-3.5 ${loading ? "motion-safe:animate-spin" : ""}`} />
              刷新
            </Button>
          </div>
        </header>

        <dl className="flex flex-wrap items-center gap-x-7 gap-y-3 rounded-xl border border-border-theme bg-elevated-bg px-4 py-3.5 text-[12px] text-text-secondary" aria-label="资源概况">
          <div>
            <dt>已安装</dt>
            <dd className="mt-0.5 tabular-nums"><strong className="text-lg font-semibold text-text-base">{byFilter.已安装.length}</strong> / {runtimes.length}</dd>
          </div>
          <div>
            <dt>已装资源大小</dt>
            <dd className="mt-0.5 text-lg font-semibold tabular-nums text-text-base">{formatBytes(installedSize)}</dd>
          </div>
          <div>
            <dt>正在下载</dt>
            <dd className="mt-0.5 tabular-nums"><strong className="text-lg font-semibold text-text-base">{byFilter.下载中.length}</strong> 项</dd>
          </div>
          {roots?.active_root && (
            <div className="min-w-0 max-w-full grow sm:ml-auto sm:max-w-72 sm:text-right">
              <dt>存储位置</dt>
              <dd className="mt-1">
                <Tooltip>
                  <TooltipTrigger render={<span tabIndex={0} />} className="block truncate rounded-md bg-hover-bg px-2 py-1 font-mono">
                    {roots.active_root}
                  </TooltipTrigger>
                  <TooltipContent className="max-w-[min(360px,90vw)] break-all">{roots.active_root}</TooltipContent>
                </Tooltip>
              </dd>
            </div>
          )}
        </dl>

        <div className="flex flex-wrap items-center justify-between gap-3">
          <div role="group" aria-label="按安装状态筛选" className="flex flex-wrap gap-0.5 rounded-xl bg-hover-bg p-1">
            {filters.map((entry) => (
              <Button key={entry} aria-pressed={filter === entry} size="sm" variant={filter === entry ? "default" : "ghost"}
                className={filter === entry ? "h-7 gap-1.5 px-2.5 text-bg-base shadow-none hover:bg-text-base" : "h-7 gap-1.5 px-2.5 hover:bg-elevated-bg"}
                onClick={() => setFilter(entry)}>
                {entry}<span className="tabular-nums opacity-65">{byFilter[entry].length}</span>
              </Button>
            ))}
          </div>
          <div className="relative min-w-40 max-w-56 flex-1">
            <Search aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 size-3.5 -translate-y-1/2 text-text-secondary" />
            <Input type="search" aria-label="搜索资源" placeholder="搜索资源" value={query} onChange={(event) => setQuery(event.target.value)}
              className="h-8 bg-elevated-bg pl-8 text-[12px] shadow-none" />
          </div>
        </div>

        <div className="space-y-5" aria-busy={loading}>
          {visibleRuntimes.length === 0 && (
            <div role="status" className="rounded-xl border border-border-theme bg-elevated-bg px-4 py-8 text-center text-[13px] text-text-secondary">
              {loading && runtimes.length === 0 ? "正在加载资源…" : needle ? "未找到匹配的资源" : "暂无资源"}
            </div>
          )}
          {groups.map((group) => {
            const entries = visibleRuntimes.filter((runtime) => resourceVisual(runtime)[0] === group);
            if (!entries.length) return null;
            return (
              <section key={group} aria-label={group}>
                <h3 className="mb-2 text-[14px] font-semibold text-text-base">{group}</h3>
                <ItemGroup className="gap-2">
                  {entries.map((runtime) => {
                    const [, logo] = resourceVisual(runtime);
                    const installing = installingIds.has(runtime.id);
                    const unavailable = unavailableReason(runtime);
                    const failed = !runtime.installed && failedIds.has(runtime.id);
                    const percent = runtimeProgressPercent(progress[runtime.id]);
                    const cancelling = cancellingIds.has(runtime.id);
                    const committing = committingIds.has(runtime.id);
                    const canUninstall = runtime.installed && runtime.install_source !== "system" && runtime.install_source !== "fallback";
                    return (
                      <Item key={runtime.id} variant="outline" size="sm" className="flex-wrap gap-x-3 gap-y-2 rounded-xl bg-elevated-bg sm:px-4">
                        <ItemMedia aria-hidden="true" className="size-9 rounded-lg bg-slate-50 p-1.5">
                          {logo ? <img alt="" src={logo} draggable={false} className="size-full object-contain" /> : <Package className="size-5 text-slate-600" />}
                        </ItemMedia>
                        <ItemContent className="min-w-0 flex-1 basis-32 gap-0.5">
                          <ItemTitle className="flex flex-wrap items-center gap-x-2 gap-y-0.5 font-semibold text-text-base">
                            <span className="min-w-0 break-words">{runtime.name}</span>
                            {runtime.installed && <span className="rounded-full bg-emerald-500/10 px-2 text-[11px] font-medium text-emerald-700 dark:text-emerald-400">已安装</span>}
                            {failed && <span className="rounded-full bg-red-500/10 px-2 text-[11px] font-medium text-red-600 dark:text-red-400">安装失败</span>}
                          </ItemTitle>
                          <ItemDescription className="tabular-nums">{runtime.version} · {formatBytes(runtime.size_bytes)}</ItemDescription>
                        </ItemContent>
                        <ItemActions className="ml-auto">
                          {installing ? (
                            <Tooltip>
                              <TooltipTrigger render={
                                <Button size="sm" variant="outline" className={`${outlineButton} min-w-12 tabular-nums`}
                                  aria-label={cancelling ? `正在取消 ${runtime.name}` : committing ? `正在安装 ${runtime.name}` : `取消下载 ${runtime.name}${percent === null ? "" : `，已下载 ${percent}%`}`}
                                  disabled={cancelling || committing} onClick={() => cancelRuntime(runtime)} />
                              }>
                                {cancelling ? "取消中…" : committing ? "安装中…" : percent === null ? "…" : `${percent}%`}
                              </TooltipTrigger>
                              <TooltipContent>取消下载</TooltipContent>
                            </Tooltip>
                          ) : runtime.installed ? (
                            <>
                              {runtime.install_path && (
                                <Button size="sm" variant="ghost" className="hover:bg-hover-bg" onClick={() => revealRuntime(runtime)}>
                                  <FolderOpen aria-hidden="true" className="size-3.5" />定位文件
                                </Button>
                              )}
                              {canUninstall && (
                                <DropdownMenu>
                                  <DropdownMenuTrigger asChild>
                                    <Button size="icon" variant="ghost" className="size-8 hover:bg-hover-bg" aria-label={`${runtime.name} 的更多操作`}>
                                      <Ellipsis aria-hidden="true" className="size-4" />
                                    </Button>
                                  </DropdownMenuTrigger>
                                  <DropdownMenuContent align="end">
                                    <DropdownMenuItem className="text-red-600 data-[highlighted]:bg-hover-bg dark:text-red-400" onSelect={() => setUninstallTarget(runtime)}>卸载…</DropdownMenuItem>
                                  </DropdownMenuContent>
                                </DropdownMenu>
                              )}
                            </>
                          ) : unavailable ? (
                            <Tooltip>
                              <TooltipTrigger render={<span tabIndex={0} aria-label={`不可用：${unavailable}`} />} className="inline-flex">
                                <Button size="sm" variant="outline" className={outlineButton} disabled>不可用</Button>
                              </TooltipTrigger>
                              <TooltipContent>{unavailable}</TooltipContent>
                            </Tooltip>
                          ) : (
                            <Button size="sm" variant="outline" className={outlineButton} onClick={() => installRuntime(runtime)}>{failed ? "重试" : "下载"}</Button>
                          )}
                        </ItemActions>
                      </Item>
                    );
                  })}
                </ItemGroup>
              </section>
            );
          })}
        </div>
      </div>

      <Dialog open={uninstallTarget !== null} onOpenChange={(open) => { if (!open && !uninstalling) setUninstallTarget(null); }}>
        <DialogContent className="max-w-sm" aria-labelledby="resource-uninstall-title" aria-describedby="resource-uninstall-description" closeOnBackdrop={!uninstalling} closeOnEscape={!uninstalling}>
          <DialogHeader><DialogTitle id="resource-uninstall-title">卸载 {uninstallTarget?.name}？</DialogTitle></DialogHeader>
          <DialogDescription id="resource-uninstall-description" className="px-6">相关功能下次使用前需要重新下载此资源。</DialogDescription>
          <DialogFooter>
            <Button autoFocus variant="outline" className={outlineButton} disabled={uninstalling} onClick={() => setUninstallTarget(null)}>取消</Button>
            <Button variant="destructive" disabled={uninstalling || !uninstallTarget} onClick={() => { if (uninstallTarget) void uninstallRuntime(uninstallTarget); }}>
              {uninstalling ? "卸载中…" : "卸载"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </TooltipProvider>
  );
}
