import { findHighlights, type HighlightLine } from "./highlightRegex";
import type { HighlightRule } from "./terminalPreferences";
const worker = self as unknown as { onmessage: ((event: MessageEvent<{ lines: HighlightLine[]; rules: HighlightRule[] }>) => void) | null; postMessage(value: unknown): void };
worker.onmessage = event => {
  try { worker.postMessage({ hits: findHighlights(event.data.lines, event.data.rules) }); }
  catch (error) { worker.postMessage({ error: String(error) }); }
};
