import { useCallback, useEffect, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { gitPush, gitPushPreview, gitPushRiskScan } from "../../api";
import type { GitOperationResult, GitPushPreview, GitPushRiskScan } from "../../types";
import { getGitUiSettings } from "./gitSettings";

interface Props {
  projectPath: string;
  onRefresh?: () => Promise<void> | void;
}

type GitPushPanelCache = {
  version: 1;
  projectPath: string;
  cachedAt: number;
  preview: GitPushPreview | null;
  riskScan: GitPushRiskScan | null;
};

const GIT_PUSH_PANEL_CACHE_PREFIX = "deepagent:git-push-panel:";

function gitPushPanelCacheKey(projectPath: string): string {
  return `${GIT_PUSH_PANEL_CACHE_PREFIX}${encodeURIComponent(projectPath)}`;
}

function readGitPushPanelCache(projectPath: string): GitPushPanelCache | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(gitPushPanelCacheKey(projectPath));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<GitPushPanelCache>;
    if (parsed.version !== 1 || parsed.projectPath !== projectPath) return null;
    return parsed as GitPushPanelCache;
  } catch {
    return null;
  }
}

function writeGitPushPanelCache(
  projectPath: string,
  preview: GitPushPreview | null,
  riskScan: GitPushRiskScan | null,
) {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(
      gitPushPanelCacheKey(projectPath),
      JSON.stringify({ version: 1, projectPath, cachedAt: Date.now(), preview, riskScan } satisfies GitPushPanelCache),
    );
  } catch {
    // Best-effort UI cache.
  }
}

export function GitPushPanel({ projectPath, onRefresh }: Props) {
  const { t } = useTranslation();
  const cached = readGitPushPanelCache(projectPath);
  const [preview, setPreview] = useState<GitPushPreview | null>(() => cached?.preview ?? null);
  const [riskScan, setRiskScan] = useState<GitPushRiskScan | null>(() => cached?.riskScan ?? null);
  const [loading, setLoading] = useState(false);
  const [pushing, setPushing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<GitOperationResult | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [nextPreview, nextRiskScan] = await Promise.all([gitPushPreview(projectPath), gitPushRiskScan(projectPath)]);
      setPreview(nextPreview);
      setRiskScan(nextRiskScan);
      writeGitPushPanelCache(projectPath, nextPreview, nextRiskScan);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [projectPath]);

  useEffect(() => {
    const nextCache = readGitPushPanelCache(projectPath);
    setResult(null);
    setError(null);
    if (nextCache) {
      setPreview(nextCache.preview);
      setRiskScan(nextCache.riskScan);
      setLoading(false);
      return;
    }
    setPreview(null);
    setRiskScan(null);
    void load();
  }, [load, projectPath]);

  const runPush = async () => {
    if (!preview || preview.blocked_reason) return;
    const settings = getGitUiSettings();
    const target = `${preview.remote ?? "origin"}/${preview.remote_branch ?? preview.current_branch ?? ""}`;
    const highRisks = riskScan?.risks.filter((risk) => risk.severity === "high") ?? [];
    if (highRisks.length > 0) {
      const ok = window.confirm(t("git.pushPanel.confirmHighRisk", { count: highRisks.length }));
      if (!ok) return;
    }
    if (settings.confirmBeforePush) {
      const ok = window.confirm(t("git.pushPanel.confirmPush", { count: preview.ahead, target }));
      if (!ok) return;
    }
    setPushing(true);
    setError(null);
    setResult(null);
    try {
      const next = await gitPush(projectPath, preview.remote, preview.remote_branch);
      setResult(next);
      if (!next.ok) {
        setError(next.stderr || next.stdout || t("git.pushPanel.pushFailed"));
      } else {
        await onRefresh?.();
        await load();
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPushing(false);
    }
  };

  const blocked = localizeGitPushBlockedReason(t, preview?.blocked_reason ?? null);
  const target = preview ? `${preview.remote ?? "origin"}/${preview.remote_branch ?? preview.current_branch ?? "-"}` : "-";

  return (
    <div className="flex h-full min-h-0 flex-col bg-white">
      <div className="flex h-11 flex-shrink-0 items-center justify-between border-b border-border-theme px-4">
        <div className="flex min-w-0 items-center">
          <FontAwesomeIcon icon={["fas", "upload"]} className="mr-2 text-text-secondary" />
          <span className="text-[14px] font-medium text-text-base">{t("git.pushPanel.title")}</span>
          {preview && (
            <span className="ml-2 truncate text-[12px] text-text-secondary">
              {preview.current_branch ?? t("git.detachedHead")} {"->"} {target}
            </span>
          )}
        </div>
        <button
          type="button"
          onClick={() => void load()}
          disabled={loading || pushing}
          className="inline-flex h-8 items-center rounded-md px-2.5 text-[12px] font-medium text-text-secondary hover:bg-gray-100 hover:text-text-base disabled:opacity-50"
        >
          <FontAwesomeIcon icon={["fas", "rotate-right"]} className="mr-1.5 text-[11px]" />
          {t("git.pushPanel.refresh")}
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto bg-gray-50/60 p-4">
        {loading ? (
          <div className="text-[13px] text-text-secondary">{t("git.pushPanel.loadingPreview")}</div>
        ) : error ? (
          <Message>{error}</Message>
        ) : preview ? (
          <div className="mx-auto flex max-w-3xl flex-col gap-3">
            <div className="rounded-lg border border-border-theme bg-white p-4">
              <div className="grid gap-3 text-[12px] text-text-secondary sm:grid-cols-2">
                <Info label={t("git.pushPanel.currentBranch")} value={preview.current_branch ?? t("git.detachedHead")} />
                <Info label={t("git.pushPanel.upstream")} value={preview.upstream ?? t("git.pushPanel.notConfigured")} />
                <Info label={t("git.pushPanel.target")} value={target} />
                <Info label={t("git.pushPanel.aheadBehind")} value={`${preview.ahead} / ${preview.behind}`} />
              </div>
              {blocked ? (
                <div className="mt-4 rounded-md bg-amber-50 px-3 py-2 text-[12px] text-amber-700">{blocked}</div>
              ) : (
                <button
                  type="button"
                  disabled={pushing}
                  onClick={runPush}
                  className="mt-4 inline-flex h-9 items-center rounded-md bg-text-base px-3 text-[12px] font-medium text-white transition-colors hover:bg-primary disabled:cursor-not-allowed disabled:bg-gray-300"
                >
                  <FontAwesomeIcon icon={["fas", "upload"]} className="mr-2 text-[11px]" />
                  {pushing ? t("git.pushPanel.pushing") : t("git.pushPanel.pushCommits", { count: preview.ahead })}
                </button>
              )}
              {result?.ok && (
                <div className="mt-3 rounded-md bg-green-50 px-3 py-2 text-[12px] text-green-700">
                  {t("git.pushPanel.pushCompleted")}
                </div>
              )}
            </div>

            <RiskScanCard scan={riskScan} />

            <div className="rounded-lg border border-border-theme bg-white">
              <div className="flex h-10 items-center justify-between border-b border-border-theme px-3">
                <div className="text-[13px] font-medium text-text-base">{t("git.pushPanel.commitsTitle")}</div>
                <div className="text-[11px] text-text-secondary">{t("git.pushPanel.commitsCount", { count: preview.commits.length })}</div>
              </div>
              {preview.commits.length === 0 ? (
                <div className="px-3 py-4 text-[13px] text-text-secondary">{t("git.pushPanel.noOutgoingCommits")}</div>
              ) : (
                <div className="divide-y divide-border-theme">
                  {preview.commits.map((commit) => (
                    <div key={commit.full_hash} className="px-3 py-2">
                      <div className="flex min-w-0 items-center gap-2">
                        <span className="font-mono text-[11px] text-text-secondary">{commit.hash}</span>
                        <span className="truncate text-[13px] font-medium text-text-base">{commit.subject}</span>
                      </div>
                      <div className="mt-0.5 flex items-center gap-2 text-[11px] text-text-secondary">
                        <span>{commit.author_name}</span>
                        <span>{formatDate(commit.date)}</span>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </div>
        ) : (
          <div className="text-[13px] text-text-secondary">{t("git.pushPanel.noPreview")}</div>
        )}
      </div>
    </div>
  );
}

function RiskScanCard({ scan }: { scan: GitPushRiskScan | null }) {
  const { t } = useTranslation();

  if (!scan) return null;
  const high = scan.risks.filter((risk) => risk.severity === "high").length;
  const medium = scan.risks.filter((risk) => risk.severity === "medium").length;
  const low = scan.risks.filter((risk) => risk.severity === "low").length;
  const blockedReason = localizeGitPushBlockedReason(t, scan.blocked_reason);

  return (
    <div className="rounded-lg border border-border-theme bg-white">
      <div className="flex h-10 items-center justify-between border-b border-border-theme px-3">
        <div className="flex items-center text-[13px] font-medium text-text-base">
          <FontAwesomeIcon icon={["fas", "shield-halved"]} className="mr-2 text-text-secondary" />
          {t("git.pushPanel.riskScanTitle")}
        </div>
        <div className="text-[11px] text-text-secondary">
          {t("git.pushPanel.riskSummary", { files: scan.scanned_files, findings: scan.risks.length })}
        </div>
      </div>
      {blockedReason ? (
        <div className="px-3 py-3 text-[12px] text-text-secondary">{blockedReason}</div>
      ) : scan.risks.length === 0 ? (
        <div className="px-3 py-3 text-[12px] text-green-700">{t("git.pushPanel.noLocalRisks")}</div>
      ) : (
        <div className="p-3">
          <div className="mb-2 flex flex-wrap gap-2 text-[11px]">
            <RiskCount label={t("git.pushPanel.severity.high")} value={high} tone="high" />
            <RiskCount label={t("git.pushPanel.severity.medium")} value={medium} tone="medium" />
            <RiskCount label={t("git.pushPanel.severity.low")} value={low} tone="low" />
          </div>
          <div className="space-y-2">
            {scan.risks.map((risk, index) => (
              <div
                key={`${risk.category}:${risk.file_path ?? ""}:${index}`}
                className={`rounded-md border px-3 py-2 text-[12px] ${riskClass(risk.severity)}`}
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="font-medium">{risk.title}</div>
                  <span className="shrink-0 text-[10px] uppercase">{localizeSeverity(t, risk.severity)}</span>
                </div>
                <div className="mt-1 text-[11px] opacity-90">{risk.detail}</div>
                {risk.file_path && (
                  <div className="mt-1 truncate font-mono text-[11px] opacity-80" title={risk.file_path}>
                    {risk.file_path}
                  </div>
                )}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function RiskCount({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone: "high" | "medium" | "low";
}) {
  const cls =
    tone === "high"
      ? "bg-red-50 text-red-600"
      : tone === "medium"
        ? "bg-amber-50 text-amber-700"
        : "bg-gray-100 text-text-secondary";
  return (
    <span className={`rounded px-1.5 py-0.5 ${cls}`}>
      {label} <span className="font-medium">{value}</span>
    </span>
  );
}

function riskClass(severity: string): string {
  if (severity === "high") return "border-red-100 bg-red-50 text-red-700";
  if (severity === "medium") return "border-amber-100 bg-amber-50 text-amber-800";
  return "border-gray-100 bg-gray-50 text-text-base";
}

function Info({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <div className="text-[11px] uppercase text-text-secondary">{label}</div>
      <div className="mt-0.5 truncate text-[13px] font-medium text-text-base" title={value}>
        {value}
      </div>
    </div>
  );
}

function Message({ children }: { children: string }) {
  return <div className="rounded-md bg-red-50 px-3 py-2 text-[12px] text-red-600">{children}</div>;
}

function formatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function localizeSeverity(t: TFunction, severity: string): string {
  if (severity === "high") return t("git.pushPanel.severity.high");
  if (severity === "medium") return t("git.pushPanel.severity.medium");
  if (severity === "low") return t("git.pushPanel.severity.low");
  return severity;
}

function localizeGitPushBlockedReason(t: TFunction, reason: string | null): string | null {
  if (!reason) return null;

  const exact: Record<string, string> = {
    "nothing to push": t("git.pushPanel.reasons.nothingToPush"),
    "not a git repository": t("git.pushPanel.reasons.notGitRepository"),
    "no upstream branch is configured": t("git.pushPanel.reasons.noUpstream"),
    "upstream has new commits; update before pushing": t("git.pushPanel.reasons.upstreamAhead"),
    "detached HEAD cannot be pushed from this view": t("git.pushPanel.reasons.detachedHead"),
    "finish or abort the active merge before pushing": t("git.pushPanel.reasons.finishMerge"),
    "finish or abort the active rebase before pushing": t("git.pushPanel.reasons.finishRebase"),
  };

  return exact[reason] ?? reason;
}
