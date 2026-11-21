import type { HighlightRule } from "./terminalPreferences";
export type HighlightLine = { line: number; text: string };
export type HighlightHit = { line: number; start: number; end: number; color: string };
/** Run exclusively inside a disposable worker; hostile regular expressions never block the UI. */
export function findHighlights(lines: HighlightLine[], rules: HighlightRule[]): HighlightHit[] {
  const hits: HighlightHit[] = [];
  for (const rule of rules.slice(0, 8)) {
    if (!rule.enabled || !rule.pattern || rule.pattern.length > 128 || !/^#[0-9a-f]{6}$/i.test(rule.color)) continue;
    const regex = new RegExp(rule.pattern, "giu");
    for (const row of lines.slice(0, 160)) {
      const text = row.text.slice(0, 2048); regex.lastIndex = 0;
      let match: RegExpExecArray | null;
      while ((match = regex.exec(text))) {
        if (match[0].length) hits.push({ line: row.line, start: match.index, end: match.index + match[0].length, color: rule.color });
        else regex.lastIndex += text.codePointAt(regex.lastIndex)! > 0xffff ? 2 : 1;
        if (hits.length >= 256) return hits;
      }
    }
  }
  return hits;
}
