// SkillInstallDialog — controlled modal that drives the
// scan → AI review → install flow for a single market skill (R4.1-R4.11).
//
// Lifecycle (controlled by `source`):
//   1. `source` becomes non-null  → kick off `skillMarketScan(githubUrl)`.
//   2. On scan success           → if `aiReviewEnabled`, subscribe to the
//      `skill-ai-review[-done]` event stream + start the AI review;
//      otherwise transition straight to "ready".
//   3. User clicks Install       → `skillMarketInstall(tempId)`; on success
//      `onClose(installed)`; on failure surface inline error and revert.
//   4. User clicks Cancel        → `skillMarketCancel(tempId)` (best-effort)
//      + `onClose(null)`. Effect-cleanup also cancels on unmount / source
//      change so a temp dir is never leaked.
//
// State machine matches the spec — the variants are public-facing because the
// install button color/label depend on it.

import { useEffect, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type {
  AiReviewResult,
  FileInfo,
  MarketSkill,
  RiskItem,
  ScanReport,
  Skill,
} from "../../types";
import {
  skillMarketAiReview,
  skillMarketAiReviewSubscribe,
  skillMarketCancel,
  skillMarketInstall,
  skillMarketScan,
} from "../../api";
import { SkillRiskBadge } from "./SkillRiskBadge";

export interface SkillInstallDialogProps {
  /** When set, the dialog is open and the parent has triggered a scan. */
  source: { githubUrl: string; skill: MarketSkill } | null;
  /** Whether the AI security review runs (driven by AppSettings). */
  aiReviewEnabled: boolean;
  /** Called when the user cancels or installs successfully. Parent should set
   *  `source` to null and (on install) refresh the installed list. */
  onClose: (installed: Skill | null) => void;
}

type ReviewFields = {
  reviewText: string;
  verdict: AiReviewResult | null;
  reviewError: string | null;
};

type Phase =
  | { kind: "idle" }
  | { kind: "scanning" }
  | ({ kind: "reviewing"; tempId: string; report: ScanReport } & ReviewFields)
  | ({ kind: "ready"; tempId: string; report: ScanReport } & ReviewFields)
  | { kind: "installing"; tempId: string; report: ScanReport }
  | { kind: "error"; message: string };

const SKILL_MD_PREVIEW_CHARS = 2000;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

function severityRank(report: ScanReport): "danger" | "warning" | "safe" {
  if (report.risks.some((r) => r.severity === "danger")) return "danger";
  if (report.risks.some((r) => r.severity === "warning")) return "warning";
  return "safe";
}

function countSeverity(risks: RiskItem[]): {
  danger: number;
  warning: number;
  safe: number;
} {
  let danger = 0;
  let warning = 0;
  let safe = 0;
  for (const r of risks) {
    if (r.severity === "danger") danger += 1;
    else if (r.severity === "warning") warning += 1;
    else safe += 1;
  }
  return { danger, warning, safe };
}

// ---------------------------------------------------------------------------
// component
// ---------------------------------------------------------------------------

export function SkillInstallDialog({
  source,
  aiReviewEnabled,
  onClose,
}: SkillInstallDialogProps) {
  const { t } = useTranslation();

  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [installError, setInstallError] = useState<string | null>(null);
  const [showAnalysis, setShowAnalysis] = useState(false);

  // Most recent successful temp_id, captured outside the phase value so the
  // effect's cleanup can cancel even mid-transition.
  const tempIdRef = useRef<string | null>(null);
  // Latest unlisten handle for `skill-ai-review[-done]`. Cleared when we
  // settle into ready / installing / error.
  const unlistenRef = useRef<(() => Promise<void>) | null>(null);

  // -------------------------------------------------------------------------
  // Scan / review pipeline. Re-runs whenever `source` becomes non-null or
  // `aiReviewEnabled` flips while the dialog is open.
  // -------------------------------------------------------------------------

  useEffect(() => {
    if (!source) {
      // Parent closed us; reset transient UI bits.
      setPhase({ kind: "idle" });
      setInstallError(null);
      setShowAnalysis(false);
      tempIdRef.current = null;
      return;
    }

    let cancelled = false;
    setInstallError(null);
    setShowAnalysis(false);

    (async () => {
      setPhase({ kind: "scanning" });
      try {
        const result = await skillMarketScan(source.githubUrl);
        if (cancelled) {
          // Effect cleanup already ran (parent flipped source to null mid-
          // scan). Drop the temp dir and bail.
          await skillMarketCancel(result.temp_id).catch(() => {});
          return;
        }
        tempIdRef.current = result.temp_id;

        if (!aiReviewEnabled) {
          setPhase({
            kind: "ready",
            tempId: result.temp_id,
            report: result.report,
            reviewText: "",
            verdict: null,
            reviewError: null,
          });
          return;
        }

        setPhase({
          kind: "reviewing",
          tempId: result.temp_id,
          report: result.report,
          reviewText: "",
          verdict: null,
          reviewError: null,
        });

        const unlisten = await skillMarketAiReviewSubscribe(
          result.temp_id,
          (payload) => {
            // Append the streamed token. Guard the kind — if we already
            // settled to "ready" / "error" we ignore late tokens.
            setPhase((prev) =>
              prev.kind === "reviewing"
                ? { ...prev, reviewText: prev.reviewText + payload.token }
                : prev
            );
          },
          (payload) => {
            setPhase((prev) => {
              if (prev.kind !== "reviewing") return prev;
              return {
                kind: "ready",
                tempId: prev.tempId,
                report: prev.report,
                reviewText: prev.reviewText,
                verdict: payload.result,
                reviewError: payload.error,
              };
            });
            // The settle event also signals the listeners are no longer
            // useful. Detach so we don't leak across reopens.
            const u = unlistenRef.current;
            unlistenRef.current = null;
            if (u) void u();
          }
        );

        if (cancelled) {
          // Cleanup already torn down the dialog while we were awaiting the
          // listen handles; release and bail.
          await unlisten();
          await skillMarketCancel(result.temp_id).catch(() => {});
          return;
        }
        unlistenRef.current = unlisten;

        try {
          await skillMarketAiReview(result.temp_id);
        } catch (e) {
          // If `start` itself rejects (e.g. no chat model configured), fall
          // through to ready with the error inline so the user can still
          // confirm or cancel.
          if (cancelled) return;
          const msg = e instanceof Error ? e.message : String(e);
          setPhase((prev) => {
            if (prev.kind !== "reviewing") return prev;
            return {
              kind: "ready",
              tempId: prev.tempId,
              report: prev.report,
              reviewText: prev.reviewText,
              verdict: null,
              reviewError: msg,
            };
          });
          const u = unlistenRef.current;
          unlistenRef.current = null;
          if (u) void u();
        }
      } catch (e) {
        if (cancelled) return;
        const msg = e instanceof Error ? e.message : String(e);
        setPhase({ kind: "error", message: msg });
        const id = tempIdRef.current;
        tempIdRef.current = null;
        if (id) await skillMarketCancel(id).catch(() => {});
      }
    })();

    return () => {
      cancelled = true;
      const u = unlistenRef.current;
      unlistenRef.current = null;
      if (u) void u();
      const id = tempIdRef.current;
      tempIdRef.current = null;
      if (id) void skillMarketCancel(id).catch(() => {});
    };
  }, [source, aiReviewEnabled]);

  // -------------------------------------------------------------------------
  // Actions
  // -------------------------------------------------------------------------

  function handleCancelClick() {
    const id = tempIdRef.current;
    tempIdRef.current = null;
    const u = unlistenRef.current;
    unlistenRef.current = null;
    if (u) void u();
    if (id) void skillMarketCancel(id).catch(() => {});
    onClose(null);
  }

  function handleInstallClick() {
    if (phase.kind !== "ready") return;
    const snapshot = phase;
    setInstallError(null);
    setPhase({
      kind: "installing",
      tempId: snapshot.tempId,
      report: snapshot.report,
    });
    void (async () => {
      try {
        const installed = await skillMarketInstall(snapshot.tempId);
        // Backend has now consumed the temp dir — clear our handle so the
        // effect cleanup doesn't try to cancel a stale id.
        tempIdRef.current = null;
        const u = unlistenRef.current;
        unlistenRef.current = null;
        if (u) void u();
        onClose(installed);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        // Roll back to the ready snapshot so the user can retry / cancel.
        setPhase(snapshot);
        setInstallError(msg);
      }
    })();
  }

  // -------------------------------------------------------------------------
  // Render
  // -------------------------------------------------------------------------

  if (!source) return null;

  const titleName = source.skill.name;

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/30 px-4">
      <div className="w-full max-w-2xl max-h-[90vh] flex flex-col rounded-2xl bg-white shadow-[0_20px_60px_rgba(15,23,42,0.18)] border border-border-theme overflow-hidden">
        {/* Header */}
        <div className="flex items-start justify-between px-6 py-4 border-b border-border-theme">
          <div className="min-w-0 flex-1 pr-3">
            <div className="text-base font-semibold text-text-base truncate">
              {t("skillInstallDialog.title", { name: titleName })}
            </div>
            <div className="text-[11px] text-text-secondary truncate mt-0.5">
              {source.githubUrl}
            </div>
          </div>
          <button
            onClick={handleCancelClick}
            disabled={phase.kind === "installing"}
            title={t("skillInstallDialog.cancel")}
            className="w-7 h-7 rounded-full border border-border-theme flex items-center justify-center text-text-secondary hover:bg-gray-50 hover:text-text-base transition-colors disabled:opacity-40 disabled:cursor-not-allowed flex-shrink-0"
          >
            <FontAwesomeIcon icon={["fas", "xmark"]} className="text-xs" />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto px-6 py-4">
          {phase.kind === "scanning" && <ScanningState />}
          {phase.kind === "error" && <ErrorState message={phase.message} />}
          {(phase.kind === "reviewing" ||
            phase.kind === "ready" ||
            phase.kind === "installing") && (
            <DialogBody
              phase={phase}
              aiReviewEnabled={aiReviewEnabled}
              showAnalysis={showAnalysis}
              onToggleAnalysis={() => setShowAnalysis((v) => !v)}
            />
          )}
        </div>

        {/* Footer */}
        <div className="px-6 py-3 border-t border-border-theme bg-gray-50/40 flex items-center justify-end gap-2">
          {installError && (
            <div className="mr-auto text-[12px] text-red-600 truncate max-w-[60%]">
              {t("skillInstallDialog.install_failed", { error: installError })}
            </div>
          )}
          <button
            onClick={handleCancelClick}
            disabled={phase.kind === "installing"}
            className="px-4 py-1.5 text-sm rounded-full border border-border-theme bg-white text-text-base hover:bg-gray-50 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {t("skillInstallDialog.cancel")}
          </button>
          <InstallButton
            phase={phase}
            aiReviewEnabled={aiReviewEnabled}
            onClick={handleInstallClick}
          />
        </div>
      </div>
    </div>
  );
}

// =============================================================================
// Sub-components
// =============================================================================

function ScanningState() {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-3 py-12 justify-center text-text-secondary text-sm">
      <FontAwesomeIcon
        icon={["fas", "circle-notch"]}
        className="animate-spin text-base"
      />
      <span>{t("skillInstallDialog.scanning")}</span>
    </div>
  );
}

function ErrorState({ message }: { message: string }) {
  return (
    <div className="py-8 text-center">
      <div className="inline-flex items-center gap-2 text-red-600 text-sm">
        <FontAwesomeIcon icon={["fas", "circle-exclamation"]} />
        <span>{message}</span>
      </div>
    </div>
  );
}

interface DialogBodyProps {
  phase:
    | ({ kind: "reviewing"; tempId: string; report: ScanReport } & ReviewFields)
    | ({ kind: "ready"; tempId: string; report: ScanReport } & ReviewFields)
    | { kind: "installing"; tempId: string; report: ScanReport };
  aiReviewEnabled: boolean;
  showAnalysis: boolean;
  onToggleAnalysis: () => void;
}

function DialogBody({
  phase,
  aiReviewEnabled,
  showAnalysis,
  onToggleAnalysis,
}: DialogBodyProps) {
  const report = phase.report;
  const sortedFiles = [...report.files].sort((a, b) =>
    a.name.localeCompare(b.name)
  );
  const totalSize = report.files.reduce((acc, f) => acc + f.size, 0);

  // Review fields are present in reviewing / ready, absent in installing.
  const review =
    phase.kind === "reviewing" || phase.kind === "ready"
      ? {
          text: phase.reviewText,
          verdict: phase.verdict,
          error: phase.reviewError,
          running: phase.kind === "reviewing",
        }
      : null;

  return (
    <div className="space-y-5">
      {aiReviewEnabled && review && (
        <AiReviewSection
          running={review.running}
          text={review.text}
          verdict={review.verdict}
          error={review.error}
          showAnalysis={showAnalysis}
          onToggleAnalysis={onToggleAnalysis}
        />
      )}
      <StaticScanSection risks={report.risks} />
      <FilesSection
        files={sortedFiles}
        count={report.files.length}
        totalSize={totalSize}
      />
      <SkillMdPreviewSection content={report.skill_md_content} />
    </div>
  );
}

interface AiReviewSectionProps {
  running: boolean;
  text: string;
  verdict: AiReviewResult | null;
  error: string | null;
  showAnalysis: boolean;
  onToggleAnalysis: () => void;
}

function AiReviewSection({
  running,
  text,
  verdict,
  error,
  showAnalysis,
  onToggleAnalysis,
}: AiReviewSectionProps) {
  const { t } = useTranslation();
  return (
    <section>
      <div className="flex items-center justify-between mb-2">
        <h3 className="text-[12px] font-semibold uppercase tracking-wide text-text-secondary">
          {t("skillInstallDialog.ai_review")}
        </h3>
        {running && (
          <span className="inline-flex items-center gap-1.5 text-[11px] text-blue-600">
            <FontAwesomeIcon
              icon={["fas", "circle-notch"]}
              className="animate-spin text-[10px]"
            />
            {t("skillInstallDialog.ai_review_running")}
          </span>
        )}
        {!running && verdict && (
          <span
            className={`inline-flex items-center gap-1 text-[11px] rounded-full px-2 py-0.5 border ${
              verdict.passed
                ? "bg-green-50 text-green-700 border-green-200"
                : "bg-red-50 text-red-700 border-red-200"
            }`}
          >
            <FontAwesomeIcon
              icon={["fas", verdict.passed ? "check" : "xmark"]}
              className="text-[10px]"
            />
            {verdict.passed
              ? t("skillInstallDialog.ai_review_passed")
              : t("skillInstallDialog.ai_review_failed")}
          </span>
        )}
      </div>

      {error && (
        <div className="text-[12px] text-red-600 bg-red-50 border border-red-200 rounded-lg px-3 py-2 mb-2">
          {t("skillInstallDialog.ai_review_error", { error })}
        </div>
      )}

      {running ? (
        <pre className="text-[11px] text-text-base whitespace-pre-wrap font-mono leading-relaxed bg-gray-50 border border-border-theme rounded-lg p-3 max-h-48 overflow-y-auto">
          {text || "\u00a0"}
        </pre>
      ) : (
        text && (
          <>
            <button
              type="button"
              onClick={onToggleAnalysis}
              className="text-[11px] text-text-secondary hover:text-text-base inline-flex items-center gap-1"
            >
              <FontAwesomeIcon
                icon={["fas", showAnalysis ? "chevron-down" : "chevron-right"]}
                className="text-[9px]"
              />
              {showAnalysis
                ? t("skillInstallDialog.ai_review_hide_details")
                : t("skillInstallDialog.ai_review_show_details")}
            </button>
            {showAnalysis && (
              <pre className="mt-2 text-[11px] text-text-base whitespace-pre-wrap font-mono leading-relaxed bg-gray-50 border border-border-theme rounded-lg p-3 max-h-48 overflow-y-auto">
                {text}
              </pre>
            )}
          </>
        )
      )}
    </section>
  );
}

function StaticScanSection({ risks }: { risks: RiskItem[] }) {
  const { t } = useTranslation();
  const counts = countSeverity(risks);
  return (
    <section>
      <h3 className="text-[12px] font-semibold uppercase tracking-wide text-text-secondary mb-2">
        {t("skillInstallDialog.static_scan")}
      </h3>
      {risks.length === 0 ? (
        <div className="text-[12px] text-text-secondary bg-gray-50 border border-border-theme rounded-lg px-3 py-2">
          {t("skillInstallDialog.no_risks")}
        </div>
      ) : (
        <>
          <div className="text-[12px] text-text-base mb-2">
            {t("skillInstallDialog.risk_count", counts)}
          </div>
          <ul className="space-y-1.5">
            {risks.map((r, i) => (
              <li
                key={`${r.file}:${r.line ?? 0}:${i}`}
                className="flex items-start gap-2 text-[12px]"
              >
                <SkillRiskBadge
                  category={r.category}
                  severity={r.severity}
                  detail={r.detail}
                  className="flex-shrink-0 mt-0.5"
                />
                <div className="min-w-0 flex-1 text-text-secondary">
                  <span className="text-text-base font-mono text-[11px]">
                    {r.file}
                    {r.line != null ? `:${r.line}` : ""}
                  </span>
                  <span className="mx-1">·</span>
                  <span>{r.detail}</span>
                </div>
              </li>
            ))}
          </ul>
        </>
      )}
    </section>
  );
}

function FilesSection({
  files,
  count,
  totalSize,
}: {
  files: FileInfo[];
  count: number;
  totalSize: number;
}) {
  const { t } = useTranslation();
  return (
    <section>
      <div className="flex items-center justify-between mb-2">
        <h3 className="text-[12px] font-semibold uppercase tracking-wide text-text-secondary">
          {t("skillInstallDialog.files")}
        </h3>
        <span className="text-[11px] text-text-secondary">
          {t("skillInstallDialog.files_summary", {
            count,
            size: formatSize(totalSize),
          })}
        </span>
      </div>
      <ul className="text-[12px] font-mono bg-gray-50 border border-border-theme rounded-lg p-2 max-h-40 overflow-y-auto divide-y divide-gray-100">
        {files.map((f) => (
          <li
            key={f.name}
            className="flex items-center justify-between py-1 px-1"
          >
            <span className="truncate text-text-base">{f.name}</span>
            <span className="text-text-secondary text-[11px] ml-3 flex-shrink-0">
              {formatSize(f.size)}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function SkillMdPreviewSection({ content }: { content: string }) {
  const { t } = useTranslation();
  return (
    <section>
      <h3 className="text-[12px] font-semibold uppercase tracking-wide text-text-secondary mb-2">
        {t("skillInstallDialog.skill_md_preview")}
      </h3>
      <pre className="text-[11px] text-text-base whitespace-pre-wrap font-mono leading-relaxed bg-gray-50 border border-border-theme rounded-lg p-3 max-h-48 overflow-y-auto">
        {content.slice(0, SKILL_MD_PREVIEW_CHARS)}
        {content.length > SKILL_MD_PREVIEW_CHARS ? "\n…" : ""}
      </pre>
    </section>
  );
}

interface InstallButtonProps {
  phase: Phase;
  aiReviewEnabled: boolean;
  onClick: () => void;
}

function InstallButton({ phase, aiReviewEnabled, onClick }: InstallButtonProps) {
  const { t } = useTranslation();

  // Default disabled label (idle / scanning / error / reviewing).
  let label: string = t("skillInstallDialog.install_warning");
  let className =
    "px-4 py-1.5 text-sm rounded-full transition-colors text-white shadow-sm";
  let disabled = true;
  let color = "bg-gray-300 text-gray-600 cursor-not-allowed";

  if (phase.kind === "installing") {
    label = t("skillInstallDialog.installing");
    color = "bg-blue-500 text-white cursor-wait";
    disabled = true;
  } else if (phase.kind === "reviewing") {
    // Block until the review settles.
    label = t("skillInstallDialog.ai_review_running");
    color = "bg-gray-300 text-gray-600 cursor-not-allowed";
    disabled = true;
  } else if (phase.kind === "ready") {
    disabled = false;
    const sev = severityRank(phase.report);
    // When AI review is enabled and didn't pass we still surface a button —
    // but we down-grade "Install Safe" to plain "Install".
    const aiPassed = aiReviewEnabled
      ? phase.verdict?.passed === true
      : true;
    if (sev === "danger") {
      label = t("skillInstallDialog.install_danger");
      color = "bg-red-500 hover:bg-red-600 text-white";
    } else if (sev === "warning") {
      label = t("skillInstallDialog.install_warning");
      color = "bg-amber-500 hover:bg-amber-600 text-white";
    } else if (aiPassed) {
      label = t("skillInstallDialog.install_safe");
      color = "bg-green-600 hover:bg-green-700 text-white";
    } else {
      // All static-scan safe but AI review verdict is not pass — neutral
      // amber so the user still sees a soft warning.
      label = t("skillInstallDialog.install_warning");
      color = "bg-amber-500 hover:bg-amber-600 text-white";
    }
  }

  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className={`${className} ${color} disabled:opacity-90`}
    >
      {label}
    </button>
  );
}
