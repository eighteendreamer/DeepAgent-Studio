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
import { HoverInfo } from "../ui/HoverInfo";

const RUNTIME_CAPABILITY_LABEL: Record<string, string> = {
  "speech-model": "语音模型",
  "speech-engine": "转写引擎",
  "video-media-tools": "视频处理",
  "doc-convert": "文档转换",
  "pdf-render": "PDF 渲染",
  "office-suite": "Office 套件",
  "vision-image-to-text": "系统视觉",
};

const RUNTIME_DESCRIPTIONS: Record<string, string> = {
  "whisper-base": "默认本地语音转文字模型，适合会议录音、语音备忘和日常转写。",
  "whisper-small": "更高质量的 Whisper 模型，体积更大，适合对准确率要求更高的录音。",
  "whisper-cli": "whisper.cpp 本地转写引擎，语音模型需要通过它在本机执行。",
  ffmpeg: "FFmpeg 与 FFprobe 视频工具。Windows/Linux LGPL 构建可按需下载；macOS 原生构建仍待完成来源与再分发校验。",
  pandoc: "增强 Markdown、docx 等文档转换能力。",
  pdfium: "增强 PDF 页面预览和栅格化渲染能力。",
  libreoffice: "用于旧版 Office 格式、高保真转换和导出能力。",
};

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
    <div className="mb-12 max-w-[700px]">
      <div className="flex items-end justify-between mb-4">
        <div>
          <h2 className="text-[15px] font-medium text-text-base mb-1">{t("settings.tabs.resources")}</h2>
          <div className="text-[12px] text-text-secondary">
            管理本地语音模型、转写引擎、文档转换、PDF 渲染和视觉模型等可选运行时。新资源安装在当前资源目录中，旧目录仅作为兼容读取来源。
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {roots && roots.fallback_roots.length > 0 && (
            <button
              type="button"
              onClick={migrateResources}
              disabled={loading}
              className="flex items-center px-3 py-1.5 rounded-lg border border-border-theme text-[12px] text-text-base hover:bg-black/5 disabled:opacity-50"
            >
              <FontAwesomeIcon icon={["fas", "download"]} className="mr-2 text-[11px]" />
              迁移资源
            </button>
          )}
          <button
            type="button"
            onClick={refresh}
            disabled={loading}
            className="flex items-center px-3 py-1.5 rounded-lg border border-border-theme text-[12px] text-text-base hover:bg-black/5 disabled:opacity-50"
          >
            <FontAwesomeIcon icon={["fas", "rotate-right"]} className={`mr-2 text-[11px] ${loading ? "animate-spin" : ""}`} />
            刷新
          </button>
        </div>
      </div>

      <div className="border border-border-theme rounded-xl shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white overflow-hidden">
        {loading && runtimes.length === 0 ? (
          <div className="p-4 text-[13px] text-text-secondary">正在加载资源列表...</div>
        ) : runtimes.length === 0 ? (
          <div className="p-4 text-[13px] text-text-secondary">暂无可管理资源。</div>
        ) : (
          runtimes.map((runtime, index) => {
            const isBusy = busyIds.has(runtime.id);
            const currentProgress = progress[runtime.id];
            const pct = runtimeProgressPercent(currentProgress);
            const installDisabled =
              isBusy ||
              runtime.installed ||
              !runtime.available_for_platform ||
              !runtime.checksum_pinned;
            const unavailableReason = !runtime.available_for_platform
              ? "当前系统不可用"
              : !runtime.checksum_pinned
              ? "等待校验值固定"
              : null;

            return (
              <div
                key={runtime.id}
                className={`p-4 ${index === runtimes.length - 1 ? "" : "border-b border-border-theme"}`}
              >
                <div className="flex items-start justify-between gap-4">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2 mb-1">
                      <span className="text-[14px] font-medium text-text-base">{runtime.name}</span>
                      <span className="rounded-md bg-gray-100 px-2 py-0.5 text-[11px] text-text-secondary">
                        {RUNTIME_CAPABILITY_LABEL[runtime.capability] ?? runtime.capability}
                      </span>
                      <span
                        className={`rounded-md px-2 py-0.5 text-[11px] ${
                          runtime.installed
                            ? "bg-green-50 text-green-600"
                            : "bg-gray-100 text-text-secondary"
                        }`}
                      >
                        {runtime.installed ? "已安装" : "未安装"}
                      </span>
                    </div>
                    <div className="text-[12px] text-text-secondary leading-relaxed">
                      {RUNTIME_DESCRIPTIONS[runtime.id] ?? "按需下载的本地运行时资源。"}
                    </div>
                    <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-text-secondary">
                      <span>ID: <code className="font-mono">{runtime.id}</code></span>
                      <span>版本: {runtime.version}</span>
                      <span>大小: {formatBytes(runtime.size_bytes)}</span>
                      {runtime.install_path && (
                        <span className="max-w-full truncate">
                          路径: {runtime.install_path}
                          {runtime.install_source === "fallback"
                            ? "（旧目录）"
                            : runtime.install_source === "system"
                              ? "（系统安装）"
                              : ""}
                        </span>
                      )}
                      {unavailableReason && <span className="text-amber-600">{unavailableReason}</span>}
                    </div>
                    {currentProgress && (
                      <div className="mt-3">
                        <div className="h-1.5 overflow-hidden rounded-full bg-gray-100">
                          <div
                            className="h-full bg-primary transition-all"
                            style={{ width: `${pct ?? 8}%` }}
                          />
                        </div>
                        <div className="mt-1 text-[11px] text-text-secondary">
                          {currentProgress.phase === "downloading" ? "下载中" : currentProgress.phase}
                          {pct !== null ? ` ${pct}%` : ""}
                          {" · "}
                          {formatBytes(currentProgress.downloaded)}
                          {currentProgress.total ? ` / ${formatBytes(currentProgress.total)}` : ""}
                        </div>
                      </div>
                    )}
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    {isBusy && !runtime.installed ? (
                      <button
                        type="button"
                        onClick={() => cancelRuntime(runtime)}
                        className="rounded-lg border border-border-theme px-3 py-1.5 text-[12px] text-text-base hover:bg-black/5"
                      >
                        取消
                      </button>
                    ) : runtime.installed ? (
                      <button
                        type="button"
                        onClick={() => uninstallRuntime(runtime)}
                        disabled={isBusy}
                        className="rounded-lg border border-red-200 bg-red-50/50 px-3 py-1.5 text-[12px] font-medium text-red-500 hover:bg-red-50 disabled:opacity-50"
                      >
                        卸载
                      </button>
                    ) : (
                      <HoverInfo content={unavailableReason ?? undefined}><button
                        type="button"
                        onClick={() => installRuntime(runtime)}
                        disabled={installDisabled}
                        className="rounded-lg bg-primary px-3 py-1.5 text-[12px] font-medium text-white hover:opacity-90 disabled:bg-gray-200 disabled:text-text-secondary"

                      >
                        下载
                      </button></HoverInfo>
                    )}
                  </div>
                </div>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
