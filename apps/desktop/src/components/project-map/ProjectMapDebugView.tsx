import { useEffect, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { projectMapOverview, projectMapRefreshDeep } from "../../api";
import type { ProjectMapOverview } from "../../types";

const DEBUG_KEY = "deepagent:project-map-debug";
const DEBUG_BUTTON_KEY = "deepagent:project-map-debug-button-visible";

export function readProjectMapDebugEnabled(): boolean {
  if (typeof window === "undefined") return false;
  return window.localStorage.getItem(DEBUG_KEY) === "1";
}

export function writeProjectMapDebugEnabled(enabled: boolean) {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(DEBUG_KEY, enabled ? "1" : "0");
  window.dispatchEvent(new CustomEvent("deepagent:project-map-debug-changed", { detail: enabled }));
}

export function readProjectMapDebugButtonVisible(): boolean {
  if (typeof window === "undefined") return false;
  return window.localStorage.getItem(DEBUG_BUTTON_KEY) === "1";
}

export function writeProjectMapDebugButtonVisible(visible: boolean) {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(DEBUG_BUTTON_KEY, visible ? "1" : "0");
  window.dispatchEvent(new CustomEvent("deepagent:project-map-debug-button-visible-changed", { detail: visible }));
}

export function ProjectMapDebugToggle({
  enabled,
  onChange,
}: {
  enabled: boolean;
  onChange: (enabled: boolean) => void;
}) {
  return (
    <button
      type="button"
      className={`h-7 rounded-md px-2 text-[11px] transition-colors ${
        enabled
          ? "bg-gray-900 text-white hover:bg-gray-800"
          : "text-text-secondary hover:bg-black/5 hover:text-text-base"
      }`}
      onClick={() => onChange(!enabled)}
    >
      Debug
    </button>
  );
}

export function ProjectMapDebugView({
  projectPath,
  compact = false,
}: {
  projectPath?: string | null;
  compact?: boolean;
}) {
  const [overview, setOverview] = useState<ProjectMapOverview | null>(null);
  const [loading, setLoading] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  const reload = () => {
    setLoading(true);
    projectMapOverview(projectPath)
      .then((next) => {
        setOverview(next);
        setMessage(null);
      })
      .catch((err) => setMessage(String(err)))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectPath]);

  const deepRebuild = async () => {
    setLoading(true);
    try {
      const result = await projectMapRefreshDeep(projectPath);
      setMessage(result.message);
      const next = await projectMapOverview(projectPath);
      setOverview(next);
    } catch (err) {
      setMessage(String(err));
    } finally {
      setLoading(false);
    }
  };

  const copyDebugInfo = () => {
    const status = overview?.status;
    const payload = {
      projectPath: projectPath ?? null,
      source: status?.source ?? null,
      status: status?.status ?? "missing",
      graphPath: status?.graph_path ?? null,
      updatedAt: status?.updated_at ?? null,
      files: status?.files ?? 0,
      nodes: status?.nodes ?? 0,
      edges: status?.edges ?? 0,
      functions: status?.functions ?? 0,
      classes: status?.classes ?? 0,
      lastError: status?.last_error ?? null,
      languages: overview?.languages ?? [],
      frameworks: overview?.frameworks ?? [],
    };
    navigator.clipboard?.writeText(JSON.stringify(payload, null, 2)).catch(() => {});
    setMessage("调试信息已复制。");
  };

  const status = overview?.status;

  return (
    <div className={compact ? "space-y-3" : "pb-20"}>
      {!compact && (
        <div className="mb-8">
          <h1 className="text-2xl font-semibold text-text-base mb-1">项目地图调试</h1>
          <p className="text-[13px] text-text-secondary">查看当前项目地图状态、路径和调试统计。</p>
        </div>
      )}

      <div className="rounded-xl border border-border-theme bg-white p-4 shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <div className="text-[13px] font-medium text-text-base">状态</div>
            <div className="mt-1 text-[12px] text-text-secondary truncate">
              {status?.graph_path ?? "未找到项目地图"}
            </div>
          </div>
          <div className="flex items-center gap-2">
            <button
              type="button"
              className="h-8 rounded-md px-3 text-[12px] text-text-secondary hover:bg-black/5 hover:text-text-base disabled:opacity-50"
              disabled={loading}
              onClick={reload}
            >
              <FontAwesomeIcon icon={["fas", "rotate-right"]} className={loading ? "animate-spin" : ""} />
            </button>
            <button
              type="button"
              className="h-8 rounded-md bg-gray-900 px-3 text-[12px] font-medium text-white hover:bg-gray-800 disabled:opacity-50"
              disabled={loading}
              onClick={deepRebuild}
            >
              重建
            </button>
          </div>
        </div>

        <div className="mt-4 grid grid-cols-2 gap-2 text-[12px]">
          <DebugItem label="source" value={status?.source ?? "missing"} />
          <DebugItem label="status" value={status?.status ?? "missing"} />
          <DebugItem label="files" value={status?.files ?? 0} />
          <DebugItem label="nodes" value={status?.nodes ?? 0} />
          <DebugItem label="edges" value={status?.edges ?? 0} />
          <DebugItem label="functions" value={status?.functions ?? 0} />
          <DebugItem label="classes" value={status?.classes ?? 0} />
          <DebugItem label="updated_at" value={status?.updated_at ?? "null"} />
        </div>

        <div className="mt-3 rounded-lg border border-border-theme bg-gray-50 p-3 text-[12px] text-text-secondary">
          <div className="truncate">project: {projectPath ?? "active project"}</div>
          <div className="mt-1 truncate">languages: {(overview?.languages ?? []).join(", ") || "-"}</div>
          <div className="mt-1 truncate">frameworks: {(overview?.frameworks ?? []).join(", ") || "-"}</div>
          {status?.last_error && <div className="mt-1 text-red-500">error: {status.last_error}</div>}
        </div>

        <div className="mt-3 flex items-center justify-between">
          <button
            type="button"
            className="h-8 rounded-md px-3 text-[12px] text-text-secondary hover:bg-black/5 hover:text-text-base"
            onClick={copyDebugInfo}
          >
            复制调试信息
          </button>
          {message && <span className="text-[11px] text-text-secondary">{message}</span>}
        </div>
      </div>
    </div>
  );
}

function DebugItem({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="rounded-lg border border-border-theme bg-gray-50 px-2 py-1.5">
      <div className="text-[10px] text-text-secondary">{label}</div>
      <div className="mt-0.5 truncate text-[12px] font-medium text-text-base">{String(value)}</div>
    </div>
  );
}
