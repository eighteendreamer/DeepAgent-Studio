import { HoverCard, HoverCardContent, HoverCardTrigger } from "./shadcn/hover-card";
import { useEffect, useState, useCallback, useRef } from "react";
import { LoaderCircle, Wallet } from "lucide-react";
import { useTranslation } from "react-i18next";

import { getBalance, SETTINGS_CHANGED_EVENT, type SettingsChangedDetail } from "../api";
import { formatBalanceAmount } from "./chat-timeline/format";
import type { Balance, BalanceInfo } from "../types";

/**
 * The selected provider's balance. Click to refresh; hover for any breakdown.
 */
export function BalanceChip({ popoverSuppressed = false }: { popoverSuppressed?: boolean }) {
  const { t } = useTranslation();
  const [balance, setBalance] = useState<Balance | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hovered, setHovered] = useState(false);
  const aborted = useRef(false);
  const generation = useRef(0);

  const refresh = useCallback(async () => {
    const requested = ++generation.current;
    setLoading(true);
    setError(null);
    setBalance(null);
    try {
      const next = await getBalance();
      if (!aborted.current && requested === generation.current) setBalance(next);
    } catch (e) {
      if (!aborted.current && requested === generation.current) {
        setBalance(null);
        setError((e as Error).message ?? String(e));
      }
    } finally {
      if (!aborted.current && requested === generation.current) setLoading(false);
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
      generation.current += 1;
      window.removeEventListener(SETTINGS_CHANGED_EVENT, onChanged);
    };
  }, [refresh]);

  const primary = balance?.infos[0];
  const label = formatLabel(primary, loading, error, t);
  const tone = pickTone(balance, error, loading);

  return (
    <HoverCard open={hovered && !popoverSuppressed} onOpenChange={(next, details) => {
      if (details.reason === "trigger-press") details.cancel();
      else setHovered(next);
    }}>
      <HoverCardTrigger delay={180} closeDelay={150} render={<button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          if (!loading) refresh();
        }}
        className={`inline-flex items-center text-[12px] font-medium cursor-pointer transition-colors ${tone}`}
      >
        {loading ? (
          <LoaderCircle className="mr-2 h-3 w-3 animate-spin" />
        ) : (
          <Wallet className="mr-2 h-3 w-3" />
        )}
        {label}
      </button>} />

      <HoverCardContent motionTone="menu" side="top" align="end" className="min-w-[240px] cursor-pointer px-3 py-2.5"
        onClick={(event) => { event.stopPropagation(); if (!loading) refresh(); }}>
        {balance && balance.infos.length > 0 ? <>
          {balance.infos.map((info, i) => (
            <div key={`${info.currency}-${i}`} className={i > 0 ? "mt-3" : ""}>
              {i > 0 && <div className="my-1.5 h-px shrink-0 bg-border-theme opacity-[0.55]" />}
              <div className="flex items-center justify-between pb-2 font-medium">
                <span>{info.currency}</span>
                <span className="tabular-nums">{formatBalanceAmount(info.currency, info.total_balance, info.unlimited)}</span>
              </div>
              {(info.granted_balance || info.topped_up_balance) && <>
              <div className="my-1.5 h-px shrink-0 bg-border-theme opacity-[0.55]" />
              <div className="flex items-center justify-between py-0.5 text-text-secondary tabular-nums">
                <span>{t("balanceChip.granted")}</span>
                <span>{formatBalanceAmount(info.currency, info.granted_balance)}</span>
              </div>
              <div className="flex items-center justify-between py-0.5 text-text-secondary tabular-nums">
                <span>{t("balanceChip.toppedUp")}</span>
                <span>{formatBalanceAmount(info.currency, info.topped_up_balance)}</span>
              </div>
              </>}
            </div>
          ))}
          {balance.is_available === false && (
            <>
              <div className="my-1.5 h-px shrink-0 bg-border-theme opacity-[0.55]" />
              <div className="pt-0.5 text-[11px] text-amber-600">{t("balanceChip.depleted")}</div>
            </>
          )}
        </> : tooltipText(balance, error, t)}
      </HoverCardContent>
    </HoverCard>
  );
}

function pickTone(
  balance: Balance | null,
  error: string | null,
  loading: boolean,
): string {
  if (loading) return "text-text-secondary";
  if (error) return "text-text-secondary hover:text-text-base";
  if (balance?.is_available === false) return "text-amber-600 hover:text-amber-700";
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
  return `${t("balanceChip.label")} ${formatBalanceAmount(primary.currency, primary.total_balance, primary.unlimited)}`;
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
