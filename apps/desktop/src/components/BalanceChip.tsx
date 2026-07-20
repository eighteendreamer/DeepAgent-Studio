import { useEffect, useState, useCallback, useRef } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";

import { getBalance, SETTINGS_CHANGED_EVENT, type SettingsChangedDetail } from "../api";
import type { Balance, BalanceInfo } from "../types";

/**
 * A compact "余额: ¥xx.xx" chip that calls DeepSeek's `/user/balance` and
 * shows the primary balance line. Click to refresh; hover to see the
 * granted/topped-up breakdown across every currency the account holds.
 */
export function BalanceChip() {
  const { t } = useTranslation();
  const [balance, setBalance] = useState<Balance | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hovered, setHovered] = useState(false);
  const aborted = useRef(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const next = await getBalance();
      if (!aborted.current) setBalance(next);
    } catch (e) {
      if (!aborted.current) {
        setBalance(null);
        setError((e as Error).message ?? String(e));
      }
    } finally {
      if (!aborted.current) setLoading(false);
    }
  }, []);

  // Load once on mount. After that, only refresh for auth/balance-relevant
  // settings changes; model / thinking-depth changes should not hit
  // DeepSeek's `/user/balance`.
  useEffect(() => {
    aborted.current = false;
    refresh();
    const onChanged = (event: Event) => {
      const detail = (event as CustomEvent<SettingsChangedDetail>).detail;
      if (!detail || detail.affectsBalance || detail.reason === "api_key") refresh();
    };
    window.addEventListener(SETTINGS_CHANGED_EVENT, onChanged);
    return () => {
      aborted.current = true;
      window.removeEventListener(SETTINGS_CHANGED_EVENT, onChanged);
    };
  }, [refresh]);

  const primary = balance?.infos[0];
  const label = formatLabel(primary, loading, error, t);
  const tone = pickTone(balance, error, loading);

  return (
    <div
      className="relative"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          if (!loading) refresh();
        }}
        title={tooltipText(balance, error, t)}
        className={`inline-flex items-center text-[12px] font-medium cursor-pointer transition-colors ${tone}`}
      >
        <FontAwesomeIcon
          icon={loading ? ["fas", "circle-notch"] : ["fas", "wallet"]}
          className={`mr-2 text-[12px] ${loading ? "animate-spin" : ""}`}
        />
        {label}
      </button>

      {hovered && balance && balance.infos.length > 0 && (
        <div className="absolute bottom-full right-0 mb-2 min-w-[220px] bg-white border border-border-theme rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] p-3 z-50 text-[12px] text-text-base">
          {balance.infos.map((info, i) => (
            <div key={`${info.currency}-${i}`} className={i > 0 ? "mt-2 pt-2 border-t border-border-theme" : ""}>
              <div className="flex items-center justify-between font-medium mb-1">
                <span>{info.currency}</span>
                <span className="tabular-nums">{formatAmount(info.currency, info.total_balance)}</span>
              </div>
              <div className="flex items-center justify-between text-text-secondary tabular-nums">
                <span>{t("balanceChip.granted")}</span>
                <span>{formatAmount(info.currency, info.granted_balance)}</span>
              </div>
              <div className="flex items-center justify-between text-text-secondary tabular-nums">
                <span>{t("balanceChip.toppedUp")}</span>
                <span>{formatAmount(info.currency, info.topped_up_balance)}</span>
              </div>
            </div>
          ))}
          {!balance.is_available && (
            <div className="mt-2 pt-2 border-t border-border-theme text-[11px] text-amber-600">
              {t("balanceChip.depleted")}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function pickTone(
  balance: Balance | null,
  error: string | null,
  loading: boolean,
): string {
  if (loading) return "text-text-secondary";
  if (error) return "text-text-secondary hover:text-text-base";
  if (balance && !balance.is_available) return "text-amber-600 hover:text-amber-700";
  return "text-text-secondary hover:text-text-base";
}

function formatLabel(
  primary: BalanceInfo | undefined,
  loading: boolean,
  error: string | null,
  t: (key: string) => string,
): string {
  if (loading && !primary) return t("balanceChip.loading");
  if (error) return t("balanceChip.unavailable");
  if (!primary) return t("balanceChip.notSet");
  return `${t("balanceChip.label")} ${formatAmount(primary.currency, primary.total_balance)}`;
}

/** Render an amount with a currency-appropriate symbol. */
function formatAmount(currency: string, value: string): string {
  const trimmed = value?.trim() ?? "";
  if (!trimmed) return "—";
  const symbol = currencySymbol(currency);
  // Round display to 2 decimals when the source has more, else keep as-is.
  const num = Number(trimmed);
  if (Number.isFinite(num)) return `${symbol}${num.toFixed(2)}`;
  return `${symbol}${trimmed}`;
}

function currencySymbol(currency: string): string {
  switch ((currency || "").toUpperCase()) {
    case "CNY":
    case "RMB":
      return "¥";
    case "USD":
      return "$";
    case "EUR":
      return "€";
    case "GBP":
      return "£";
    default:
      return `${currency} `.trimEnd() + " ";
  }
}

function tooltipText(
  balance: Balance | null,
  error: string | null,
  t: (key: string) => string,
): string {
  if (error) return `${t("balanceChip.unavailable")}: ${error}`;
  if (!balance) return t("balanceChip.refreshHint");
  return t("balanceChip.refreshHint");
}
