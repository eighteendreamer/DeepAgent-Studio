const CNY_SYMBOL = "\uFFE5";

export function formatMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds % 60);
  return `${minutes}m${rest}s`;
}

export function formatTokens(n: number): string {
  if (n < 1000) return `${n}`;
  return `${(n / 1000).toFixed(n < 10000 ? 1 : 0)}k`;
}

export function formatCny(n: number): string {
  if (n <= 0) return `${CNY_SYMBOL}0`;
  if (n < 0.01) return `${CNY_SYMBOL}${n.toFixed(6)}`;
  if (n < 1) return `${CNY_SYMBOL}${n.toFixed(4)}`;
  return `${CNY_SYMBOL}${n.toFixed(2)}`;
}

export const cnySymbol = CNY_SYMBOL;
