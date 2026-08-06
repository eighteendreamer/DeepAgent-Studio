// SkillsMarketProviderConfig — gear-button popover that lets users configure
// the SkillsMP API key, test the connection, and clear a saved key (R9.2,
// R9.4, R9.5, R9.6).
//
// Positioning is layout-driven (no rect math): the parent wraps both the
// trigger and this component in a `relative` container, and the popover hangs
// off the trigger via `absolute right-0 top-full mt-2`. Dismissal is driven by
// (1) the close button, (2) Escape, and (3) any mousedown that lands outside
// the popover root.
//
// Security notes (Property 8 / R9.4):
//   - The backend `skill_market_get_api_key` only exposes `{ has_user_key,
//     source }` — never the key value. The popover renders presence/source
//     only and never tries to display a stored key.
//   - The "Save" path calls `skillMarketSetApiKey` with the user-typed value
//     and then immediately clears the draft so the input is empty after save.

import { useEffect, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type { ApiKeyInfo, SkillsMpKeySource, TestKeyResult } from "../../types";
import {
  skillMarketClearApiKey,
  skillMarketGetApiKey,
  skillMarketSetApiKey,
  skillMarketTestKey,
} from "../../api";
import { message } from "../message";

export interface SkillsMarketProviderConfigProps {
  /** When true, the popover is mounted and visible. */
  open: boolean;
  /** Called when the popover should close (close button, click-outside,
   *  Escape key). */
  onClose: () => void;
}

export function SkillsMarketProviderConfig({
  open,
  onClose,
}: SkillsMarketProviderConfigProps) {
  const { t } = useTranslation();
  const rootRef = useRef<HTMLDivElement | null>(null);

  const [keyInfo, setKeyInfo] = useState<ApiKeyInfo | null>(null);
  const [keyDraft, setKeyDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<TestKeyResult | null>(null);
  const [inlineError, setInlineError] = useState<string | null>(null);
  const [clearing, setClearing] = useState(false);

  // ------------------------------------------------------------------
  // Lifecycle: refetch keyInfo on open + reset transient state on close.
  // ------------------------------------------------------------------
  useEffect(() => {
    if (!open) {
      // Reset transient state so re-opening starts fresh.
      setKeyDraft("");
      setTestResult(null);
      setInlineError(null);
      return;
    }
    let cancelled = false;
    void skillMarketGetApiKey()
      .then((info) => {
        if (!cancelled) setKeyInfo(info);
      })
      .catch((e) => {
        if (!cancelled) {
          setInlineError(e instanceof Error ? e.message : String(e));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  // ------------------------------------------------------------------
  // Click-outside + Escape dismiss.
  // ------------------------------------------------------------------
  useEffect(() => {
    if (!open) return;
    function onMouseDown(e: MouseEvent) {
      const root = rootRef.current;
      if (!root) return;
      if (e.target instanceof Node && !root.contains(e.target)) {
        onClose();
      }
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("mousedown", onMouseDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onMouseDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, onClose]);

  if (!open) return null;

  // ------------------------------------------------------------------
  // Actions.
  // ------------------------------------------------------------------
  async function refetchInfo() {
    try {
      const info = await skillMarketGetApiKey();
      setKeyInfo(info);
    } catch (e) {
      setInlineError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleSave() {
    const trimmed = keyDraft.trim();
    if (trimmed.length === 0) {
      setInlineError(t("skillsmp.empty_key_error"));
      return;
    }
    setInlineError(null);
    setSaving(true);
    try {
      await skillMarketSetApiKey(trimmed);
      setKeyDraft("");
      setTestResult(null);
      await refetchInfo();
      message.success(t("skillsmp.saved"));
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setInlineError(msg);
    } finally {
      setSaving(false);
    }
  }

  async function handleTest() {
    setInlineError(null);
    setTesting(true);
    setTestResult(null);
    try {
      const r = await skillMarketTestKey();
      setTestResult(r);
    } catch (e) {
      setTestResult({
        ok: false,
        daily_remaining: null,
        error: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setTesting(false);
    }
  }

  async function handleClear() {
    if (!keyInfo?.has_user_key) return;
    const ok = window.confirm(t("skillsmp.clear_key_confirm"));
    if (!ok) return;
    setClearing(true);
    setInlineError(null);
    try {
      await skillMarketClearApiKey();
      setTestResult(null);
      await refetchInfo();
      message.success(t("skillsmp.cleared"));
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setInlineError(msg);
    } finally {
      setClearing(false);
    }
  }

  // ------------------------------------------------------------------
  // Render.
  // ------------------------------------------------------------------
  const hasCustomKey = keyInfo?.has_user_key === true;

  return (
    <div
      ref={rootRef}
      role="dialog"
      aria-label={t("skillsmp.title")}
      className="absolute right-0 top-full mt-2 z-30 w-80 bg-white rounded-xl shadow-[0_8px_28px_rgb(0,0,0,0.16)] border border-border-theme p-4"
    >
      {/* Header */}
      <div className="flex items-start justify-between mb-2">
        <div>
          <div className="text-sm font-semibold text-text-base">
            {t("skillsmp.title")}
          </div>
          <p className="text-[11px] text-text-secondary mt-0.5 leading-relaxed">
            {t("skillsmp.description")}
          </p>
        </div>
        <button
          onClick={onClose}
          className="text-text-secondary hover:text-text-base ml-2"
          title={t("skillsmp.close")}
          aria-label={t("skillsmp.close")}
        >
          <FontAwesomeIcon icon={["fas", "xmark"]} className="text-xs" />
        </button>
      </div>

      <div className="border-t border-border-theme my-3" />

      {/* Status badge */}
      {keyInfo && (
        <div className="mb-3">
          <StatusBadge source={keyInfo.source} t={t} />
        </div>
      )}

      {/* API key input */}
      <label className="block text-xs font-medium text-text-base mb-1">
        {t("skillsmp.api_key_label")}
      </label>
      <input
        type="password"
        autoComplete="off"
        spellCheck={false}
        placeholder={t("skillsmp.api_key_placeholder")}
        className="w-full bg-gray-50 border border-border-theme rounded-md px-2.5 py-1.5 text-sm text-text-base outline-none focus:border-gray-300 focus:bg-white transition-colors"
        value={keyDraft}
        onChange={(e) => {
          setKeyDraft(e.target.value);
          if (inlineError) setInlineError(null);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !saving) {
            e.preventDefault();
            void handleSave();
          }
        }}
        disabled={saving}
      />

      {/* External links */}
      <div className="flex items-center gap-3 mt-1.5">
        <a
          href="https://skillsmp.com/auth/login"
          target="_blank"
          rel="noopener noreferrer"
          className="text-[11px] text-blue-600 hover:underline inline-flex items-center gap-1"
        >
          {t("skillsmp.get_api_key")}
          <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="text-[9px]" />
        </a>
        <a
          href="https://skillsmp.com/docs/api"
          target="_blank"
          rel="noopener noreferrer"
          className="text-[11px] text-blue-600 hover:underline inline-flex items-center gap-1"
        >
          {t("skillsmp.view_docs")}
          <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="text-[9px]" />
        </a>
      </div>

      {/* Inline error */}
      {inlineError && (
        <div className="mt-2 text-[11px] text-red-600 bg-red-50 border border-red-200 rounded-md px-2 py-1.5">
          {inlineError}
        </div>
      )}

      {/* Action buttons */}
      <div className="flex items-center gap-2 mt-3">
        <button
          onClick={handleSave}
          disabled={saving || keyDraft.trim().length === 0}
          className="flex-1 px-3 py-1.5 text-xs rounded-md bg-text-base text-white hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed transition-opacity"
        >
          {saving ? t("skillsmp.saving") : t("skillsmp.save")}
        </button>
        <button
          onClick={handleTest}
          disabled={testing}
          className="flex-1 px-3 py-1.5 text-xs rounded-md bg-black/5 text-text-base hover:bg-black/5 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
        >
          {testing ? t("skillsmp.testing") : t("skillsmp.test_connection")}
        </button>
      </div>

      {/* Test result banner */}
      {testResult && <TestResultBanner result={testResult} t={t} />}

      {/* Clear key (only when custom key is saved) */}
      {hasCustomKey && (
        <button
          onClick={handleClear}
          disabled={clearing}
          className="mt-3 w-full text-xs text-red-600 hover:bg-red-50 rounded-md py-1.5 transition-colors disabled:opacity-50"
        >
          {clearing ? t("skillsmp.clearing") : t("skillsmp.clear_key")}
        </button>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

function StatusBadge({
  source,
  t,
}: {
  source: SkillsMpKeySource;
  t: ReturnType<typeof useTranslation>["t"];
}) {
  const { label, classes } =
    source === "user"
      ? {
          label: t("skillsmp.using_user_key"),
          classes: "bg-blue-50 border-blue-200 text-blue-700",
        }
      : source === "builtin"
        ? {
            label: t("skillsmp.using_builtin_key"),
            classes: "bg-green-50 border-green-200 text-green-700",
          }
        : {
            label: t("skillsmp.using_anonymous"),
            classes: "bg-amber-50 border-amber-200 text-amber-700",
          };

  return (
    <span
      className={`inline-flex items-center text-[11px] rounded-full px-2 py-0.5 border ${classes}`}
    >
      {label}
    </span>
  );
}

function TestResultBanner({
  result,
  t,
}: {
  result: TestKeyResult;
  t: ReturnType<typeof useTranslation>["t"];
}) {
  if (result.ok) {
    return (
      <div className="mt-3 text-[11px] text-green-700 bg-green-50 border border-green-200 rounded-md px-2 py-1.5">
        <FontAwesomeIcon icon={["fas", "circle-check"]} className="mr-1.5" />
        {t("skillsmp.test_ok")}
        {result.daily_remaining !== null && (
          <span className="ml-1.5 text-text-secondary">
            ({t("skillsmp.daily_remaining", { n: result.daily_remaining })})
          </span>
        )}
      </div>
    );
  }
  return (
    <div className="mt-3 text-[11px] text-red-700 bg-red-50 border border-red-200 rounded-md px-2 py-1.5">
      <FontAwesomeIcon icon={["fas", "circle-info"]} className="mr-1.5" />
      {t("skillsmp.test_failed")}
      {result.error && (
        <span className="ml-1.5 text-text-secondary break-words">
          {result.error}
        </span>
      )}
    </div>
  );
}
