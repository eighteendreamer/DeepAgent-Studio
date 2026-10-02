import { useEffect, useState, useCallback, useRef } from "react";
import { ArrowDown, ArrowUp, Zap, Coins, Wallet } from "lucide-react";
import { getCostSummary, getBalance, SETTINGS_CHANGED_EVENT } from "../api";
import type { CostSummary, Balance } from "../types";

interface Props {
  sessionId: string | null;
}

function formatTokens(value: number): string {
  if (value >= 1_000_000) {
    const m = value / 1_000_000;
    return `${Number.isInteger(m) ? m.toFixed(0) : m.toFixed(1)}M`;
  }
  if (value >= 1_000) {
    const k = value / 1_000;
    return `${Number.isInteger(k) ? k.toFixed(0) : k.toFixed(1)}k`;
  }
  return String(Math.max(0, Math.round(value)));
}

function formatCny(value: number): string {
  return `¥${value.toFixed(4)}`;
}

export function SessionUsageBar({ sessionId }: Props) {
  const [summary, setSummary] = useState<CostSummary | null>(null);
  const [balance, setBalance] = useState<Balance | null>(null);
  const [liveCache, setLiveCache] = useState<{ hit: number; miss: number } | null>(null);
  const [loading, setLoading] = useState(false);
  const unlistenRef = useRef<(() => void) | null>(null);

  const fetchSummary = useCallback(async () => {
    if (!sessionId) return;
    setLoading(true);
    try {
      const data = await getCostSummary(sessionId);
      setSummary(data);
    } catch (err) {
      console.error("fetch cost summary failed:", err);
    } finally {
      setLoading(false);
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

  useEffect(() => {
    fetchSummary();
    fetchBalance();
  }, [fetchSummary, fetchBalance]);

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
        const unlisten = await mod.listen<{ type: string; prompt_cache_hit_tokens?: number; prompt_cache_miss_tokens?: number }>(
          "chat://event",
          (event) => {
            if (event.payload.type === "usage") {
              const hit = event.payload.prompt_cache_hit_tokens ?? 0;
              const miss = event.payload.prompt_cache_miss_tokens ?? 0;
              setLiveCache({ hit, miss });
              setTimeout(fetchSummary, 300);
            } else if (event.payload.type === "cost_recorded") {
              setTimeout(fetchSummary, 300);
            }
          }
        );
        unlistenRef.current = unlisten;
      } catch (err) {
        // Browser or non-Tauri environment
      }
    };
    setupListener();
    return () => {
      unlistenRef.current?.();
    };
  }, [sessionId, fetchSummary]);

  if (!sessionId) return null;
  if (loading && !summary) return null;

  const cacheHitRate =
    liveCache && liveCache.hit + liveCache.miss > 0
      ? (liveCache.hit / (liveCache.hit + liveCache.miss)) * 100
      : null;

  return (
    <div className="mt-1 flex min-h-5 items-center justify-between gap-3 px-6 text-[11px] text-text-secondary opacity-80 tabular-nums">
      <span className="font-semibold text-text-tertiary">会话统计</span>
      <div className="flex flex-1 items-center justify-evenly gap-3">
        {summary && (
          <>
            <span className="text-text-tertiary">
              <ArrowDown size={11} className="mr-0.5 inline" />
              输入 {formatTokens(summary.input_tokens)}
            </span>
            <span className="text-text-tertiary">
              <ArrowUp size={11} className="mr-0.5 inline" />
              输出 {formatTokens(summary.output_tokens)}
            </span>
            {summary.cache_hit_tokens > 0 && (
              <span className="font-medium text-green-600">
                <Zap size={11} className="mr-0.5 inline" />
                缓存命中 {formatTokens(summary.cache_hit_tokens)}
              </span>
            )}
            {cacheHitRate !== null && (
              <span className="text-text-tertiary">命中率 {cacheHitRate.toFixed(1)}%</span>
            )}
            <span>
              <Coins size={11} className="mr-0.5 inline" />
              {formatCny(summary.session_cost)}
            </span>
          </>
        )}
        {balance && balance.is_available && balance.infos.length > 0 && (
          <span className="text-text-tertiary">
            <Wallet size={11} className="mr-0.5 inline" />
            余额 ¥{parseFloat(balance.infos[0].total_balance).toFixed(2)}
          </span>
        )}
      </div>
    </div>
  );
}
