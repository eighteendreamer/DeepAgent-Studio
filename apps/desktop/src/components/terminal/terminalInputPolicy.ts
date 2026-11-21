const MAX_PASTE_BYTES = 256 * 1024;
export function inspectPaste(text: string): { confirm: boolean; reasons: string[]; text: string } {
  if (new TextEncoder().encode(text).length > MAX_PASTE_BYTES) throw new Error("Paste exceeds the 256 KiB limit");
  const reasons: string[] = [];
  if (/[\r\n\u2028\u2029]/.test(text)) reasons.push("multiline");
  if (/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]/.test(text)) reasons.push("controls");
  if (/(?:\brm\s+[^\r\n]*-[^\s]*[rf]|\b(?:sudo|mkfs|dd|shutdown|reboot|format|diskpart)\b|\b(?:curl|wget)\b[^\r\n]*\|[^\r\n]*\b(?:sh|bash|zsh)\b|\b(?:remove-item|invoke-expression|iex)\b|:\(\)\s*\{)/i.test(text)) reasons.push("dangerous");
  if (text.length > 32 * 1024) reasons.push("large");
  return { confirm: reasons.length > 0, reasons, text: text.replace(/\r\n?/g, "\n").replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]/g, "") };
}
export function bracketPaste(text: string, bracketed: boolean): string {
  const safe = inspectPaste(text).text;
  return bracketed ? `\x1b[200~${safe}\x1b[201~` : safe.replace(/\n/g, "\r");
}
