import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useEffect, useState } from "react";
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
import { Progress } from "../shadcn/progress";
import { HoverInfo } from "../ui/HoverInfo";

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "未知大小";
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
  const [busyIds, setBusyIds] = useState<Set<string>>(new Set());
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

  const setBusy = (id: string, value: boolean) => {
    setBusyIds((prev) => {
      const next = new Set(prev);
      if (value) next.add(id);
      else next.delete(id);
      return next;
    });
  };

  const installRuntime = async (runtime: RuntimeStatus) => {
    setBusy(runtime.id, true);
    const unlisten = await runtimeProgressSubscribe(runtime.id, (p) => {
      setProgress((prev) => ({ ...prev, [runtime.id]: p }));
    });
    try {
      await runtimeInstall(runtime.id);
      message.success(`${runtime.name} 已安装`);
      refresh();
    } catch (e) {
      console.error("runtime_install failed:", e);
      message.error(`${runtime.name} 安装失败`);
    } finally {
      unlisten();
      setBusy(runtime.id, false);
      setProgress((prev) => {
        const next = { ...prev };
        delete next[runtime.id];
        return next;
      });
    }
  };

  const uninstallRuntime = async (runtime: RuntimeStatus) => {
    if (!window.confirm(`卸载 ${runtime.name}？\n\n卸载后相关功能下次使用前需要重新下载。`)) {
      return;
    }
    setBusy(runtime.id, true);
    try {
      await runtimeUninstall(runtime.id);
      message.success(`${runtime.name} 已卸载`);
      refresh();
    } catch (e) {
      console.error("runtime_uninstall failed:", e);
      message.error(`${runtime.name} 卸载失败`);
    } finally {
      setBusy(runtime.id, false);
    }
  };

  const cancelRuntime = async (runtime: RuntimeStatus) => {
    try {
      await runtimeCancel(runtime.id);
      message.info(`已请求取消 ${runtime.name}`);
    } catch (e) {
      console.error("runtime_cancel failed:", e);
      message.error("取消失败");
    }
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

  return (
    <div className="mb-12">
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-lg font-semibold text-text-base">{t("settings.tabs.resources")}</h2>
        <div className="flex flex-wrap items-center gap-2">
          {roots && roots.fallback_roots.length > 0 && (
            <Button variant="outline" size="sm" className="bg-elevated-bg shadow-none hover:bg-hover-bg" onClick={migrateResources} disabled={loading}>
              <FontAwesomeIcon icon={["fas", "download"]} className="text-[11px]" />
              迁移资源
            </Button>
          )}
          <Button variant="outline" size="sm" className="bg-elevated-bg shadow-none hover:bg-hover-bg" onClick={refresh} disabled={loading}>
            <FontAwesomeIcon icon={["fas", "rotate-right"]} className={loading ? "animate-spin" : undefined} />
            刷新
          </Button>
        </div>
      </div>

      <div className="rounded-xl border border-border-theme bg-elevated-bg" aria-busy={loading}>
        {runtimes.length === 0 ? (
          <div className="px-4 py-6 text-center text-[13px] text-text-secondary">
            {loading ? "正在加载资源…" : "暂无资源"}
          </div>
        ) : (
          <ul className="divide-y divide-border-theme">
            {runtimes.map((runtime) => {
              const isBusy = busyIds.has(runtime.id);
              const isInstalling = isBusy && !runtime.installed;
              const currentProgress = progress[runtime.id];
              const pct = runtimeProgressPercent(currentProgress);
              const unavailableReason = !runtime.available_for_platform
                ? "当前系统不可用"
                : !runtime.checksum_pinned
                  ? "等待校验值固定"
                  : undefined;
              const status = runtime.installed
                ? "已安装"
                : isInstalling
                  ? "安装中"
                  : unavailableReason
                    ? "不可下载"
                    : "未安装";

              return (
                <li key={runtime.id} className="px-4 py-2.5">
                  <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
                    <div className="min-w-0 flex-1 basis-36">
                      <div className="break-words text-[14px] font-medium text-text-base">{runtime.name}</div>
                      <div className="mt-0.5 flex flex-wrap items-center gap-x-1.5 text-[12px] tabular-nums text-text-secondary">
                        <span className="min-w-0 break-words">{runtime.version}</span>
                        <span aria-hidden="true">·</span>
                        <span>{formatBytes(runtime.size_bytes)}</span>
                      </div>
                    </div>
                    <div className="ml-auto flex shrink-0 items-center gap-3">
                      <span className="inline-flex min-w-[5em] items-center gap-1.5 text-[12px] text-text-secondary">
                        <span aria-hidden="true" className={`size-1.5 rounded-full ${runtime.installed ? "bg-emerald-500" : isInstalling ? "bg-primary" : "bg-border-theme"}`} />
                        {status}
                      </span>
                      {isInstalling ? (
                        <Button variant="outline" size="sm" className="w-14 bg-elevated-bg shadow-none hover:bg-hover-bg" onClick={() => cancelRuntime(runtime)}>
                          取消
                        </Button>
                      ) : runtime.installed ? (
                        <Button variant="outline" size="sm" className="w-14 bg-elevated-bg text-red-600 shadow-none hover:bg-hover-bg dark:text-red-400" onClick={() => uninstallRuntime(runtime)} disabled={isBusy}>
                          卸载
                        </Button>
                      ) : (
                        <HoverInfo content={unavailableReason}>
                          <Button size="sm" className="w-14 text-bg-base shadow-none hover:bg-text-base hover:opacity-80" onClick={() => installRuntime(runtime)} disabled={Boolean(unavailableReason)}>
                            下载
                          </Button>
                        </HoverInfo>
                      )}
                    </div>
                  </div>
                  {isInstalling && currentProgress && (
                    <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[12px] tabular-nums text-text-secondary">
                      <Progress aria-label={`${runtime.name} 下载进度`} value={pct} className="h-1 min-w-16 flex-1 bg-hover-bg" />
                      <span>
                        {formatBytes(currentProgress.downloaded)}
                        {currentProgress.total ? ` / ${formatBytes(currentProgress.total)}` : ""}
                        {pct !== null ? ` · ${pct}%` : ""}
                      </span>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}
