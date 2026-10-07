import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowDown, ArrowUp, Zap, Coins, Wallet } from "lucide-react";
import { getCostSummary, getBalance, unwrapRuntimeEvent, SETTINGS_CHANGED_EVENT } from "../api";
import { formatCny, formatTokens } from "./chat-timeline/format";
import type { Balance, CostSummary } from "../types";

interface Props {
  sessionId: string | null;
  /** A run is streaming in this session, so live usage deltas are meaningful. */
  busy?: boolean;
}

/**
 * Usage seen since the last settled pull. The ledger row is written once per run
 * at completion while `usage` arrives per model request, so the bar grows from
 * this overlay and is replaced — not added to — when the run's own row lands.
 */
interface LiveUsage {
  input: number;
  output: number;
  cacheHit: number;
  cacheMiss: number;
  cost: number;
}

const EMPTY_LIVE: LiveUsage = { input: 0, output: 0, cacheHit: 0, cacheMiss: 0, cost: 0 };

function tokenCount(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

export function SessionUsageBar({ sessionId, busy = false }: Props) {
  const [summary, setSummary] = useState<CostSummary | null>(null);
  const [live, setLive] = useState<LiveUsage>(EMPTY_LIVE);
  const [balance, setBalance] = useState<Balance | null>(null);
  const [loading, setLoading] = useState(false);
  const unlistenRef = useRef<(() => void) | null>(null);
  // Latest-request-wins: a slow reply for a session the user left must not
  // overwrite the numbers of the session they switched to.
  const generation = useRef(0);
  const busyRef = useRef(busy);
  busyRef.current = busy;

  // `settleLive` says whether the pull covers the run still in flight. Only the
  // run's own ledger row does; a side call (compaction summary, session-note
  // pass) books a row of its own while the run is mid-flight, and dropping the
  // overlay then made the bar lose the totals the ledger has not received yet.
  const fetchSummary = useCallback(async (settleLive = true) => {
    if (!sessionId) return;
    const requested = ++generation.current;
    setLoading(true);
    try {
      const data = await getCostSummary(sessionId);
      if (requested !== generation.current) return;
      setSummary(data);
      if (settleLive) setLive(EMPTY_LIVE);
    } catch (err) {
      console.error("fetch cost summary failed:", err);
    } finally {
      if (requested === generation.current) setLoading(false);
    }
  }, [sessionId]);

  const fetchBalance = useCallback(async () => {
    try {
      const data = await getBalance();
      setBalance(data);
    } catch (err) {
      console.error("fetch balance failed:", err);
    }
  }, []);

  // Every session has its own ledger: drop the previous session's numbers and
  // invalidate its in-flight reply instead of painting them until the new pull
  // resolves.
  useEffect(() => {
    generation.current += 1;
    setSummary(null);
    setLive(EMPTY_LIVE);
    setLoading(false);
  }, [sessionId]);

  useEffect(() => {
    fetchSummary();
  }, [fetchSummary]);

  useEffect(() => {
    fetchBalance();
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
    const setupListener = async () => {
      try {
        const mod = await import("@tauri-apps/api/event");
        const unlisten = await mod.listen<unknown>("chat://event", (message) => {
          const event = unwrapRuntimeEvent(message.payload);
          if (!event) return;
          if (event.type === "cost_recorded") {
            // The backend writes the ledger row before emitting this event, so
            // one pull here is authoritative — no polling timer needed. Only the
            // run's own row covers the in-flight overlay; a side-call row must
            // not settle it, or the bar drops totals still outside the ledger.
            void fetchSummary(String(event.origin ?? "run") !== "side_call");
          } else if (event.type === "usage" && busyRef.current) {
            setLive((prev) => ({
              input: prev.input + tokenCount(event.prompt_tokens),
              output: prev.output + tokenCount(event.completion_tokens),
              cacheHit: prev.cacheHit + tokenCount(event.prompt_cache_hit_tokens),
              cacheMiss: prev.cacheMiss + tokenCount(event.prompt_cache_miss_tokens),
              cost: prev.cost + tokenCount(event.cost_yuan),
            }));
          }
        });
        unlistenRef.current = unlisten;
      } catch (err) {
        // Browser preview or non-Tauri environment: the settled pull still works.
      }
    };
    setupListener();
    return () => {
      unlistenRef.current?.();
      unlistenRef.current = null;
    };
  }, [sessionId, fetchSummary]);

  if (!sessionId) return null;
  if (loading && !summary) return null;

  const totals: LiveUsage = {
    input: (summary?.input_tokens ?? 0) + live.input,
    output: (summary?.output_tokens ?? 0) + live.output,
    cacheHit: (summary?.cache_hit_tokens ?? 0) + live.cacheHit,
    cacheMiss: (summary?.cache_miss_tokens ?? 0) + live.cacheMiss,
    cost: (summary?.session_cost ?? 0) + live.cost,
  };
  const cacheTotal = totals.cacheHit + totals.cacheMiss;
  const sessionCacheHitRate = cacheTotal > 0 ? (totals.cacheHit / cacheTotal) * 100 : null;

  return (
    <div className="absolute bottom-0 left-0 right-0 flex items-center justify-center px-6 text-[11px] text-text-secondary opacity-80 tabular-nums" style={{ height: '24px' }}>
      <div className="flex items-center justify-evenly gap-4 max-w-4xl w-full">
        {summary && (
          <>
            <span className="text-text-tertiary">
              <ArrowUp size={11} className="mr-0.5 inline" />
              累计输入 {formatTokens(totals.input)}
            </span>
            <span className="text-text-tertiary">
              <ArrowDown size={11} className="mr-0.5 inline" />
              累计输出 {formatTokens(totals.output)}
            </span>
            {sessionCacheHitRate !== null && (
              <span className="font-medium text-green-600">
                <Zap size={11} className="mr-0.5 inline" />
                缓存命中 {sessionCacheHitRate.toFixed(1)}%
              </span>
            )}
            <span>
              <Coins size={11} className="mr-0.5 inline" />
              累计消耗 {formatCny(totals.cost)}
            </span>
          </>
        )}
        {balance && balance.is_available && balance.infos.length > 0 && (
          <span className="text-text-tertiary">
            <Wallet size={11} className="mr-0.5 inline" />
            余额 {formatCny(parseFloat(balance.infos[0].total_balance))}
          </span>
        )}
      </div>
    </div>
  );
}
