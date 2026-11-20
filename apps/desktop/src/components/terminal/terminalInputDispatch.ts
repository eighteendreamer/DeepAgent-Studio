import type { TerminalViewHandle } from "./terminalViewTypes";

export type TerminalInputTarget = { id: string; label: string; handle?: TerminalViewHandle };
/** The captured handle fences confirmation dialogs against closed/replaced sessions. */
export async function dispatchTerminalInput(targets: TerminalInputTarget[], current: (id: string) => TerminalViewHandle | undefined, text: string, keys: boolean, execute: boolean): Promise<string[]> {
  const results = await Promise.allSettled(targets.map(async target => {
    if (!target.handle || current(target.id) !== target.handle) throw new Error("Terminal target changed; review the input again");
    if (keys) await target.handle.send(text);
    else await target.handle.paste(text, execute);
  }));
  return results.flatMap((result, i) => result.status === "rejected" ? [targets[i].label + ": " + String(result.reason)] : []);
}
