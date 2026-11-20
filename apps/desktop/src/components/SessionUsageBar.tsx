import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowDown, ArrowUp, Zap, Coins, Wallet } from "lucide-react";
import { getSessionUsageTotals, getBalance, unwrapRuntimeEvent, SETTINGS_CHANGED_EVENT } from "../api";
import { formatBalanceAmount, formatUsd, formatTokens } from "./chat-timeline/format";
import type { Balance } from "../types";
import type { UsageCounts } from "./settings/usageStatistics";

interface Props {
  sessionId: string | null;
}

export function SessionUsageBar({ sessionId }: Props) {
  const [summary, setSummary] = useState<UsageCounts | null>(null);
  const [balance, setBalance] = useState<Balance | null>(null);
  const [loading, setLoading] = useState(false);
  const unlistenRef = useRef<(() => void) | null>(null);
  // Latest-request-wins: a slow reply for a session the user left must not
  // overwrite the numbers of the session they switched to.
  const generation = useRef(0);
  const balanceGeneration = useRef(0);
  const fetchSummary = useCallback(async () => {
    if (!sessionId) return;
    const requested = ++generation.current;
    setLoading(true);
    try {
      const data = await getSessionUsageTotals(sessionId);
      if (requested !== generation.current) return;
      setSummary(data);
    } catch (err) {
      console.error("fetch cost summary failed:", err);
    } finally {
      if (requested === generation.current) setLoading(false);
    }
  }, [sessionId]);

  const fetchBalance = useCallback(async () => {
    const requested = ++balanceGeneration.current;
    setBalance(null);
    try {
      const data = await getBalance();
      if (requested === balanceGeneration.current) setBalance(data);
    } catch (err) {
      if (requested !== balanceGeneration.current) return;
      setBalance(null);
      console.error("fetch balance failed:", err);
    }
  }, []);

  // Every session has its own ledger: drop the previous session's numbers and
  // invalidate its in-flight reply instead of painting them until the new pull
  // resolves.
  useEffect(() => {
    generation.current += 1;
    setSummary(null);
    setLoading(false);
  }, [sessionId]);

  useEffect(() => {
    fetchSummary();
  }, [fetchSummary]);

  useEffect(() => {
    fetchBalance();
    return () => { balanceGeneration.current += 1; };
  }, [fetchBalance]);

  useEffect(() => {
    const handleSettingsChanged = (event: CustomEvent) => {
      if (event.detail?.affectsBalance) {
        fetchBalance();
      }
    };
    window.addEventListener(SETTINGS_CHANGED_EVENT, handleSettingsChanged as EventListener);
    return () => {
      window.removeEventListener(SETTINGS_CHANGED_EVENT, handleSettingsChanged as EventListener);
    };
  }, [fetchBalance]);

  useEffect(() => {
    if (typeof window === "undefined" || !sessionId) return;
    let disposed = false;
    const setupListener = async () => {
      try {
        const mod = await import("@tauri-apps/api/event");
        const unlisten = await mod.listen<unknown>("chat://event", (message) => {
          const event = unwrapRuntimeEvent(message.payload);
          if (!event) return;
          // Both side and main calls are already booked before the notice.
          // Retrieve the durable totals, without adding a second live overlay.
          if (event.type === "cost_recorded" || event.type === "run_completed"
            || event.type === "run_failed" || event.type === "run_cancelled") void fetchSummary();
        });
        if (disposed) unlisten();
        else unlistenRef.current = unlisten;
      } catch (err) {
        // Browser preview or non-Tauri environment: the settled pull still works.
      }
    };
    setupListener();
    return () => {
      disposed = true;
      unlistenRef.current?.();
      unlistenRef.current = null;
    };
  }, [sessionId, fetchSummary]);

  if (!sessionId) return null;
  if (loading && !summary) return null;

  const input = summary?.input ?? 0;
  const output = summary?.output ?? 0;
  const hit = summary?.cacheRead ?? 0;
  const cost = summary?.costUsd;
  const cacheRate = input > 0 ? (100 * hit / input).toFixed(1) : null;

  return (
    <div className="absolute bottom-0 left-0 right-0 flex items-center justify-center px-6 text-[11px] text-text-secondary opacity-80 tabular-nums" style={{ height: '24px' }}>
      <div className="flex items-center justify-evenly gap-4 max-w-4xl w-full">
        {summary && (
          <>
            <span className="text-text-tertiary">
              <ArrowUp size={11} className="mr-0.5 inline" />
              累计输入 {formatTokens(input)}
            </span>
            <span className="text-text-tertiary">
              <ArrowDown size={11} className="mr-0.5 inline" />
              累计输出 {formatTokens(output)}
            </span>
            {cacheRate !== null && (
              <span className="font-medium text-green-600">
                <Zap size={11} className="mr-0.5 inline" />
                缓存命中 {cacheRate}%
              </span>
            )}
            <span>
              <Coins size={11} className="mr-0.5 inline" />
              累计消耗 {cost == null ? "未定价" : `${summary?.unpricedRequests ? "≥" : "≈"}${formatUsd(cost)}`}
            </span>
          </>
        )}
        {balance && balance.infos.length > 0 && (
          <span className="text-text-tertiary">
            <Wallet size={11} className="mr-0.5 inline" />
            余额 {formatBalanceAmount(balance.infos[0].currency, balance.infos[0].total_balance, balance.infos[0].unlimited)}
          </span>
        )}
      </div>
    </div>
  );
}
