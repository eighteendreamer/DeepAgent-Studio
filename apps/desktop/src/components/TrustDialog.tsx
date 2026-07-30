import { useEffect, useState } from "react";
import { projectTrustStatus, setProjectTrust } from "../api";

/**
 * §6.2 per-project trust prompt. When trust enforcement is active
 * (`DEEPAGENT_PROJECT_TRUST` env or the "项目信任网关" setting) and the entered
 * project directory is NOT yet trusted, this dialog asks the user to trust it
 * before commands auto-run. Until trusted, the backend `TrustGuardHook`
 * escalates bash/shell to approval — this dialog is the grant UX for that gate.
 *
 * Trust applies to the directory and its descendants (the backend checks parent
 * directories), mirroring Claude Code's per-project trust.
 *
 * No-op (renders nothing) when enforcement is off or the project is already
 * trusted, so it never nags in the common case.
 */
export function TrustDialog({ projectPath }: { projectPath: string | null }) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    if (!projectPath) {
      setOpen(false);
      return;
    }
    projectTrustStatus(projectPath)
      .then((status) => {
        if (cancelled) return;
        // Only prompt when the gate is actually active AND the project is
        // untrusted — otherwise stay silent.
        setOpen(status.enforced && !status.trusted);
      })
      .catch(() => {
        if (!cancelled) setOpen(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectPath]);

  if (!open || !projectPath) return null;

  const grant = async () => {
    setBusy(true);
    setError(null);
    try {
      await setProjectTrust(projectPath, true);
      setOpen(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const dismiss = () => {
    // Leave the project untrusted: the backend keeps escalating bash to
    // approval (fail-safe). Just close the prompt for this entry.
    setOpen(false);
  };

  return (
    <div
      className="fixed inset-0 z-[1000] flex items-center justify-center bg-black/40"
      role="dialog"
      aria-modal="true"
      aria-label="项目信任确认"
    >
      <div className="w-[460px] max-w-[90vw] rounded-xl bg-white shadow-xl border border-border-theme p-6">
        <div className="text-[16px] font-semibold text-text-base mb-2">
          是否信任此项目？
        </div>
        <div className="text-[13px] text-text-secondary leading-relaxed mb-1">
          你正在进入一个尚未信任的项目目录。信任网关已开启：在信任之前，即使是白名单命令，
          bash/shell 也会先请求你的批准（防止未知项目自动执行）。
        </div>
        <div className="text-[12px] text-text-secondary font-mono break-all bg-gray-50 border border-border-theme rounded-md px-2 py-1.5 my-3">
          {projectPath}
        </div>
        <div className="text-[12px] text-text-secondary leading-relaxed mb-4">
          信任将应用于该目录及其子目录。你可以稍后在设置中关闭"项目信任网关"，或撤销信任。
        </div>
        {error && (
          <div className="text-[12px] text-red-600 mb-3">保存失败：{error}</div>
        )}
        <div className="flex items-center justify-end gap-3">
          <button
            type="button"
            onClick={dismiss}
            disabled={busy}
            className="px-3.5 py-1.5 text-[13px] rounded-lg border border-border-theme text-text-secondary hover:text-text-base hover:bg-gray-50 transition-colors disabled:opacity-50"
          >
            暂不信任
          </button>
          <button
            type="button"
            onClick={grant}
            disabled={busy}
            className="px-3.5 py-1.5 text-[13px] rounded-lg bg-blue-500 text-white hover:bg-blue-600 transition-colors disabled:opacity-50"
          >
            {busy ? "保存中…" : "信任此项目"}
          </button>
        </div>
      </div>
    </div>
  );
}
