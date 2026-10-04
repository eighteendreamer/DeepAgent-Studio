const CNY_SYMBOL = "\uFFE5";

export function formatMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds % 60);
  return `${minutes}m${rest}s`;
}

/**
 * The app's one token formatter: the per-turn footer, the session totals bar and
 * the context-capacity ring all render the same number, so they must not carry
 * separate rounding rules (they previously disagreed — `16k` vs `15.8k`).
 */
export function formatTokens(value: number): string {
  const n = Math.max(0, Math.round(value));
  if (n < 1000) return `${n}`;
  // 999_500 rounds to "1M" rather than the "1000k" a plain 1e6 cutoff produces.
  if (n < 999_500) return `${formatUnit(n / 1000)}k`;
  return `${formatUnit(n / 1_000_000)}M`;
}

/** One decimal place, without a trailing `.0`. */
function formatUnit(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}

export function formatCny(n: number): string {
  if (n <= 0) return `${CNY_SYMBOL}0`;
  if (n < 0.01) return `${CNY_SYMBOL}${n.toFixed(6)}`;
  if (n < 1) return `${CNY_SYMBOL}${n.toFixed(4)}`;
  return `${CNY_SYMBOL}${n.toFixed(2)}`;
}

export const cnySymbol = CNY_SYMBOL;
