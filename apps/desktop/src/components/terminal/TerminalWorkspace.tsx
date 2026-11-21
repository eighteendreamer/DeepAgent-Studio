import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { sshStatus, type TerminalTarget } from "../../api";
import { TerminalPlugin } from "../plugins/TerminalPlugin";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Input } from "../shadcn/input";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "../shadcn/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";
import { collectPanes, initialLayout, moveTab, removeTab, setSplitRatio, splitPane, updatePane, validateLayout, type SplitDirection, type TerminalLayout } from "./terminalLayout";
import { inspectPaste } from "./terminalInputPolicy";
import { dispatchTerminalInput } from "./terminalInputDispatch";
import { readTerminalPreferences, updateHistory, type TerminalPreferences } from "./terminalPreferences";
import { downloadArtifact, parseCast, type TerminalCast } from "./terminalRecording";
import type { TerminalMetadataEvent } from "./terminalMetadata";
import type { IntegrationShell } from "./terminalShellIntegration";
import type { TerminalViewHandle, TerminalViewStatus } from "./terminalViewTypes";
import { TerminalPlayback } from "./TerminalPlayback";
import { TerminalPreferencesPanel } from "./TerminalPreferencesPanel";

type Tab = { id: string; name: string; endpoint: TerminalTarget; cwd?: string };
type SavedLayout = { version: 1; layout: TerminalLayout; tabs: Tab[] };
type Meta = { title?: string; cwd?: string; bell?: boolean; integration?: string; command?: string; exitCode?: number | null };
type Rect = { x: number; y: number; w: number; h: number };
type DialogMode = "history" | "quick" | "highlight" | "lock" | "integration" | "broadcast" | "record" | "close" | "restoreLayout" | null;
type InputDecision = { text: string; targets: { tab: Tab; handle: TerminalViewHandle | undefined }[]; execute: boolean; reasons: string[] };
export type TerminalOpenIntent = { token: string; endpoint: TerminalTarget; cwd?: string };
const DRAG_TYPE = "application/x-deepagent-terminal-tab";
const scopeOf = (endpoint: TerminalTarget) => endpoint.kind === "ssh" ? `ssh:${endpoint.connectionId}` : "local";
function createTab(endpoint: TerminalTarget, name: string, cwd?: string): Tab { return { id: crypto.randomUUID(), endpoint, name, cwd }; }
function readSaved(key: string): SavedLayout | null {
  try {
    const raw = localStorage.getItem(key); if (!raw || raw.length > 64 * 1024) return null;
    const value = JSON.parse(raw), layout = validateLayout(value.layout);
    if (value.version !== 1 || !layout || !Array.isArray(value.tabs) || value.tabs.length > 24) return null;
    const tabs = value.tabs as Tab[];
    if (tabs.some(tab => typeof tab.id !== "string" || typeof tab.name !== "string" || tab.name.length > 160 || !tab.endpoint || !["ssh", "local"].includes(tab.endpoint.kind) || tab.endpoint.kind === "ssh" && typeof tab.endpoint.connectionId !== "string" || tab.cwd != null && (typeof tab.cwd !== "string" || /[\x00-\x1f]/.test(tab.cwd)))) return null;
    const ids = collectPanes(layout).flatMap(pane => pane.tabs);
    if (ids.length !== tabs.length || new Set(tabs.map(tab => tab.id)).size !== tabs.length || ids.some(id => !tabs.some(tab => tab.id === id))) return null;
    return { version: 1, layout, tabs };
  } catch { return null; }
}
function geometry(layout: TerminalLayout, rect: Rect = { x: 0, y: 0, w: 100, h: 100 }): { panes: Map<string, Rect>; splits: { id: string; axis: "horizontal" | "vertical"; ratio: number; rect: Rect }[] } {
  if (layout.kind === "pane") return { panes: new Map([[layout.id, rect]]), splits: [] };
  const horizontal = layout.axis === "horizontal", ratio = layout.ratio;
  const first = geometry(layout.first, { ...rect, w: horizontal ? rect.w * ratio : rect.w, h: horizontal ? rect.h : rect.h * ratio });
  const second = geometry(layout.second, { x: horizontal ? rect.x + rect.w * ratio : rect.x, y: horizontal ? rect.y : rect.y + rect.h * ratio, w: horizontal ? rect.w * (1 - ratio) : rect.w, h: horizontal ? rect.h : rect.h * (1 - ratio) });
  return { panes: new Map([...first.panes, ...second.panes]), splits: [...first.splits, ...second.splits, { id: layout.id, axis: layout.axis, ratio, rect }] };
}

export function TerminalWorkspace({ mode = "local", connectionId = null, active = true, openIntent, onDirectoryChange }: { mode?: "local" | "remote"; connectionId?: string | null; active?: boolean; openIntent?: TerminalOpenIntent; onDirectoryChange?: (endpoint: TerminalTarget, path: string) => void }) {
  const { t } = useTranslation(); const endpoint: TerminalTarget = mode === "remote" && connectionId ? { kind: "ssh", connectionId } : { kind: "local" };
  const layoutKey = `deepagent.terminal.layout.v1:${scopeOf(endpoint)}`;
  const [first] = useState(() => createTab(endpoint, t("terminalWorkbench.terminal")));
  const [tabs, setTabs] = useState<Tab[]>([first]), [layout, setLayout] = useState(() => initialLayout(first.id)), [focused, setFocused] = useState(first.id);
  const [saved, setSaved] = useState(() => readSaved(layoutKey));
  const [statuses, setStatuses] = useState<Record<string, TerminalViewStatus>>({}), [metadata, setMetadata] = useState<Record<string, Meta>>({});
  const [endpointLabels, setEndpointLabels] = useState<Record<string, string>>({});
  const [preferences, setPreferences] = useState(() => readTerminalPreferences(localStorage));
  const [dialog, setDialog] = useState<DialogMode>(null), [error, setError] = useState(""), [password, setPassword] = useState(""), [confirmation, setConfirmation] = useState("");
  const [lockAll, setLockAll] = useState(false), [hideOutput, setHideOutput] = useState(true), [shell, setShell] = useState<IntegrationShell>("bash");
  const [broadcast, setBroadcast] = useState<string[]>([]), [decision, setDecision] = useState<InputDecision | null>(null), [cast, setCast] = useState<TerminalCast | null>(null);
  const handles = useRef(new Map<string, TerminalViewHandle>()), surface = useRef<HTMLDivElement>(null), fileInput = useRef<HTMLInputElement>(null), castInput = useRef<HTMLInputElement>(null);
  const lastIntent = useRef(""), saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null), bellTimes = useRef(new Map<string, number>());
  const tab = tabs.find(row => row.id === focused) ?? tabs[0], handle = handles.current.get(tab.id), status = statuses[tab.id], meta = metadata[tab.id];
  const panes = collectPanes(layout), positions = useMemo(() => geometry(layout), [layout]);
  const isReady = status?.state === "running", label = (row: Tab) => `${metadata[row.id]?.title || row.name} · ${endpointLabels[scopeOf(row.endpoint)] ?? scopeOf(row.endpoint)}`;
  const showError = (e: unknown) => setError(String(e));
  useEffect(() => {
    let current = true;
    const ids = [...new Set(tabs.flatMap(row => row.endpoint.kind === "ssh" ? [row.endpoint.connectionId] : []))];
    void Promise.allSettled(ids.map(async id => { const value = await sshStatus(id); if (current && value) setEndpointLabels(labels => ({ ...labels, ["ssh:" + id]: `${value.username}@${value.host}:${value.port} · ${id}` })); }));
    return () => { current = false; };
  }, [tabs]);
  useEffect(() => {
    try { localStorage.setItem("deepagent.terminal.preferences.v1", JSON.stringify(preferences)); } catch (e) { showError(e); }
  }, [preferences]);
  useEffect(() => {
    saveTimer.current = setTimeout(() => { try { localStorage.setItem(layoutKey, JSON.stringify({ version: 1, layout, tabs })); } catch (e) { showError(e); } }, 500);
    return () => { if (saveTimer.current) clearTimeout(saveTimer.current); };
  }, [layout, tabs, layoutKey]);
  function addTab(direction?: SplitDirection, target: TerminalTarget = tab.endpoint, cwd?: string) {
    if (tabs.length >= 24) { showError(t("terminalWorkbench.tabLimit")); return; }
    const next = createTab(target, t("terminalWorkbench.terminal"), cwd); const pane = panes.find(p => p.tabs.includes(focused)) ?? panes[0];
    try {
      const nextLayout = direction ? splitPane(layout, pane.id, direction, next.id) : updatePane(layout, pane.id, p => ({ ...p, tabs: [...p.tabs, next.id], active: next.id }));
      setLayout(nextLayout); setTabs(rows => [...rows, next]); setFocused(next.id);
    } catch (e) { showError(e); }
  }
  useEffect(() => {
    if (openIntent && openIntent.token !== lastIntent.current) { lastIntent.current = openIntent.token; addTab(undefined, openIntent.endpoint, openIntent.cwd); }
  }, [openIntent]);
  function focus(id: string, paneId?: string) {
    setFocused(id); setMetadata(rows => ({ ...rows, [id]: { ...rows[id], bell: false } }));
    if (paneId) setLayout(current => updatePane(current, paneId, p => ({ ...p, active: id })));
    handles.current.get(id)?.focus();
    const row = tabs.find(value => value.id === id), cwd = metadata[id]?.cwd;
    if (row && cwd) onDirectoryChange?.(row.endpoint, cwd);
  }
  function metadataEvent(row: Tab, event: TerminalMetadataEvent) {
    if (event.kind === "directory") { setMetadata(rows => ({ ...rows, [row.id]: { ...rows[row.id], cwd: event.path } })); if (row.id === focused) onDirectoryChange?.(row.endpoint, event.path); }
    else if (event.kind === "title") setMetadata(rows => ({ ...rows, [row.id]: { ...rows[row.id], title: event.title } }));
    else if (event.kind === "bell") {
      const now = Date.now(); if (now - (bellTimes.current.get(row.id) ?? 0) < 1000) return; bellTimes.current.set(row.id, now);
      setMetadata(rows => ({ ...rows, [row.id]: { ...rows[row.id], bell: true } }));
    } else if (event.kind === "integration") setMetadata(rows => ({ ...rows, [row.id]: { ...rows[row.id], integration: event.status } }));
    else if (event.kind === "commandStart") setMetadata(rows => ({ ...rows, [row.id]: { ...rows[row.id], command: event.command, exitCode: undefined } }));
    else if (event.kind === "commandEnd") {
      setMetadata(rows => ({ ...rows, [row.id]: { ...rows[row.id], command: event.command, exitCode: event.exitCode } }));
      setPreferences(current => current.historyEnabled ? { ...current, history: updateHistory(current.history, event, scopeOf(row.endpoint)) } : current);
    }
  }
  async function dispatch(input: InputDecision, keys = false) {
    const failures = await dispatchTerminalInput(input.targets.map(target => ({ id: target.tab.id, label: label(target.tab), handle: target.handle })), id => handles.current.get(id), input.text, keys, input.execute);
    if (failures.length) setError(failures.join("\n"));
  }
  function requestInput(id: string, text: string, source: "keys" | "paste" | "direct", execute = false) {
    const sourceTab = tabs.find(row => row.id === id); if (!sourceTab) return;
    const ids = source !== "direct" && broadcast.includes(id) ? broadcast : [id];
    const targets = ids.map(target => tabs.find(row => row.id === target)).filter((row): row is Tab => !!row).map(row => ({ tab: row, handle: handles.current.get(row.id) }));
    try {
      const paste = source === "paste" ? inspectPaste(text) : { text, confirm: false, reasons: [] };
      const input = { text: paste.text, targets, execute, reasons: paste.reasons };
      if (paste.confirm || execute) setDecision(input); else void dispatch(input, source !== "paste");
    } catch (e) { showError(e); }
  }
  function closeTab() {
    const next = removeTab(layout, tab.id); if (!next) return;
    setLayout(next); setTabs(rows => rows.filter(row => row.id !== tab.id)); setFocused(collectPanes(next)[0].active); setBroadcast(ids => ids.filter(id => id !== tab.id));
    setStatuses(rows => { const result = { ...rows }; delete result[tab.id]; return result; }); setMetadata(rows => { const result = { ...rows }; delete result[tab.id]; return result; }); bellTimes.current.delete(tab.id); setDialog(null);
  }
  async function applyLock() {
    if (!status?.locked && (!password || password !== confirmation)) { showError(t("terminalWorkbench.passwordMismatch")); return; }
    const targets = lockAll ? tabs : [tab]; const unlocking = !!status?.locked;
    const results = await Promise.allSettled(targets.map(async row => {
      const current = handles.current.get(row.id); if (!current) throw new Error(`${label(row)}: ${t("terminalWorkbench.unavailable")}`);
      if (unlocking) { if (statuses[row.id]?.locked) await current.unlock(password); }
      else if (!statuses[row.id]?.locked) await current.lock(password, hideOutput);
    }));
    setPassword(""); setConfirmation("");
    const failed = results.flatMap((result, i) => result.status === "rejected" ? [`${label(targets[i])}: ${String(result.reason)}`] : []);
    if (failed.length) setError(failed.join("\n")); else { setError(""); setDialog(null); }
  }
  function restoreLayout() {
    if (!saved || tabs.some(row => statuses[row.id]?.locked)) return;
    const map = new Map(saved.tabs.map(row => [row.id, crypto.randomUUID()]));
    const remap = (node: TerminalLayout): TerminalLayout => node.kind === "pane"
      ? { ...node, id: crypto.randomUUID(), tabs: node.tabs.map(id => map.get(id)!), active: map.get(node.active)! }
      : { ...node, id: crypto.randomUUID(), first: remap(node.first), second: remap(node.second) };
    setLayout(remap(saved.layout)); setTabs(saved.tabs.map(row => ({ ...row, id: map.get(row.id)! })));
    setFocused(map.get(saved.tabs[0].id)!); setSaved(null); setBroadcast([]);
    setStatuses({}); setMetadata({}); setDialog(null);
  }
  function exportRecording(log = false) {
    const data = handle?.recording(); if (!data) return;
    const name = `terminal-${new Date().toISOString().replace(/[:.]/g, "-")}`;
    if (log) { const text = parseCast(data).events.filter(e => e[1] === "o").map(e => e[2]).join("").replace(/\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))/g, ""); downloadArtifact(`${name}.log`, text, "text/plain;charset=utf-8"); }
    else downloadArtifact(`${name}.cast`, data, "application/x-asciicast");
  }
  return <div className="flex h-full min-h-0 flex-col bg-bg-base">
    <div className="flex shrink-0 flex-wrap items-center gap-1 border-b border-border-theme px-2 py-1">
      <Button size="sm" variant="ghost" onClick={() => addTab()}>{t("terminalWorkbench.newTab")}</Button>
      {(["left", "right", "top", "bottom"] as const).map(direction => <Button key={direction} size="sm" variant="ghost" disabled={panes.length >= 8} onClick={() => addTab(direction, tab.endpoint, metadata[tab.id]?.cwd)} aria-label={t(`terminalWorkbench.split.${direction}`)}>{({ left: "◧", right: "◨", top: "⬒", bottom: "⬓" })[direction]}</Button>)}
      <Button size="sm" variant={broadcast.length ? "outline" : "ghost"} onClick={() => setDialog("broadcast")}>{t("terminalWorkbench.broadcast")}{broadcast.length ? ` · ${broadcast.length}` : ""}</Button>
      {(["history", "quick", "highlight"] as const).map(item => <Button key={item} size="sm" variant="ghost" onClick={() => setDialog(item)}>{t(`terminalWorkbench.${item}`)}</Button>)}
      <Button size="sm" variant="ghost" disabled={!isReady || status?.locked} onClick={() => setDialog("integration")}>{t("terminalWorkbench.integration")}</Button>
      <Button size="sm" variant={status?.recording ? "outline" : "ghost"} disabled={!isReady || status?.locked} onClick={() => { if (status?.recording) handle?.record(false); else setDialog("record"); }}>{t(`terminalWorkbench.${status?.recording ? "stopRecording" : "record"}`)}</Button>
      <Button size="sm" variant="ghost" disabled={!status?.hasRecording} onClick={() => exportRecording()}>{t("terminalWorkbench.exportCast")}</Button>
      <Button size="sm" variant="ghost" disabled={!status?.hasRecording} onClick={() => exportRecording(true)}>{t("terminalWorkbench.exportLog")}</Button>
      <Button size="sm" variant="ghost" disabled={!status?.hasRecording} onClick={() => handle?.discardRecording()}>{t("terminalWorkbench.discardRecording")}</Button>
      <Button size="sm" variant="ghost" onClick={() => castInput.current?.click()}>{t("terminalWorkbench.replay")}</Button>
      <Button size="sm" variant={status?.locked ? "outline" : "ghost"} disabled={!isReady} onClick={() => setDialog("lock")}>{t(`terminalWorkbench.${status?.locked ? "unlock" : "lock"}`)}</Button>
      {saved && <Button size="sm" variant="ghost" disabled={tabs.some(row => statuses[row.id]?.locked)} onClick={() => setDialog("restoreLayout")}>{t("terminalWorkbench.restoreLayout")}</Button>}
    </div>
    <div className="flex shrink-0 items-center gap-3 overflow-hidden border-b border-border-theme px-3 py-1 text-[11px] text-text-secondary">
      <span className="truncate">{scopeOf(tab.endpoint)}{meta?.cwd ? ` · ${meta.cwd}` : ""}</span><span>{t(`terminalWorkbench.integrationStates.${meta?.integration ?? "unavailable"}`)}</span>
      {status?.recording && <strong className="text-red-500">● {t("terminalWorkbench.recording")}</strong>}{broadcast.length > 0 && <strong className="text-amber-600">{t("terminalWorkbench.broadcastActive")}</strong>}
      {meta?.command && <span className="truncate">{meta.command}{meta.exitCode !== undefined ? ` · ${meta.exitCode ?? "?"}` : ""}</span>}
    </div>
    {status?.zmodem && <div className="flex flex-wrap items-center gap-2 border-b border-border-theme px-3 py-1 text-xs"><strong>ZMODEM</strong><span>{status.zmodem.name ?? status.zmodem.role} {status.zmodem.bytes ?? 0} / {status.zmodem.size ?? "?"}</span>
      {status.zmodem.phase === "detected" && <Button size="sm" onClick={() => handle?.confirmTransfer()}>{t("terminalWorkbench.confirmTransfer")}</Button>}
      {status.zmodem.phase === "offer" && <><Button size="sm" onClick={() => void handle?.acceptTransfer().catch(showError)}>{t("terminalWorkbench.receive")}</Button><Button size="sm" variant="ghost" onClick={() => handle?.skipTransfer()}>{t("terminalWorkbench.skip")}</Button></>}
      {status.zmodem.role === "send" && status.zmodem.phase === "waiting" && <Button size="sm" onClick={() => fileInput.current?.click()}>{t("terminalWorkbench.chooseFiles")}</Button>}
      <Button size="sm" variant="outline" onClick={() => handle?.cancelTransfer()}>{t("terminalWorkbench.cancel")}</Button>
    </div>}
    {error && <div role="alert" className="flex shrink-0 items-start justify-between gap-2 whitespace-pre-wrap border-b border-border-theme bg-red-500/10 px-3 py-1 text-xs text-red-500"><span>{error}</span><Button size="sm" variant="ghost" onClick={() => setError("")}>×</Button></div>}
    <div ref={surface} className="relative min-h-0 flex-1 overflow-hidden">
      {panes.map(pane => { const rect = positions.panes.get(pane.id)!; return <div key={pane.id} className="absolute z-10 flex h-8 items-center gap-1 overflow-x-auto border border-border-theme bg-elevated-bg px-1" style={{ left: `${rect.x}%`, top: `${rect.y}%`, width: `${rect.w}%` }} onDragOver={e => { if (e.dataTransfer.types.includes(DRAG_TYPE)) e.preventDefault(); }} onDrop={e => { const id = e.dataTransfer.getData(DRAG_TYPE); if (tabs.some(row => row.id === id)) { e.preventDefault(); setLayout(current => moveTab(current, id, pane.id)); focus(id); } }}>
        {pane.tabs.map(id => { const row = tabs.find(value => value.id === id)!; return <Button key={id} size="sm" variant={pane.active === id ? "secondary" : "ghost"} className="min-w-0 shrink-0 gap-1 px-2" draggable onDragStart={e => { e.dataTransfer.setData(DRAG_TYPE, id); e.dataTransfer.effectAllowed = "move"; }} onClick={() => focus(id, pane.id)} title={label(row)}><span className="max-w-36 truncate">{metadata[id]?.title || row.name}</span>{metadata[id]?.bell && <span aria-label={t("terminalWorkbench.bell")}>●</span>}{statuses[id]?.locked && <span aria-label={t("terminalWorkbench.locked")}>🔒</span>}{statuses[id]?.recording && <span className="text-red-500">●</span>}</Button>; })}
        <Button variant="ghost" size="sm" disabled={tabs.length === 1} aria-label={t("terminalWorkbench.close")} onClick={() => { focus(pane.active); setDialog("close"); }}>×</Button>
      </div>; })}
      {/* Stable sibling roots: splitting, dragging and resizing only change geometry. */}
      {tabs.map(row => { const pane = panes.find(p => p.tabs.includes(row.id))!, rect = positions.panes.get(pane.id)!; const visible = pane.active === row.id; return <div key={row.id} data-terminal-tab={row.id} className="absolute border border-border-theme" onFocusCapture={() => { if (focused !== row.id) focus(row.id); }} style={{ left: `${rect.x}%`, top: `calc(${rect.y}% + 32px)`, width: `${rect.w}%`, height: `calc(${rect.h}% - 32px)`, display: visible ? undefined : "none" }}>
        <TerminalPlugin mode={row.endpoint.kind === "ssh" ? "remote" : "local"} connectionId={row.endpoint.kind === "ssh" ? row.endpoint.connectionId : null} initialCwd={row.cwd} active={active && visible && focused === row.id} highlights={preferences.highlights} onControls={controls => { if (controls) handles.current.set(row.id, controls); else handles.current.delete(row.id); }} onStatus={value => setStatuses(current => ({ ...current, [row.id]: value }))} onMetadata={event => metadataEvent(row, event)} onUserInput={(text, source) => requestInput(row.id, text, source)} onUnlockRequest={() => { focus(row.id); setDialog("lock"); }} />
      </div>; })}
      {positions.splits.map(split => { const horizontal = split.axis === "horizontal", rect = split.rect; const style = horizontal ? { left: `calc(${rect.x + rect.w * split.ratio}% - 3px)`, top: `${rect.y}%`, width: 6, height: `${rect.h}%`, cursor: "col-resize" } : { left: `${rect.x}%`, top: `calc(${rect.y + rect.h * split.ratio}% - 3px)`, height: 6, width: `${rect.w}%`, cursor: "row-resize" }; return <div key={split.id} role="separator" tabIndex={0} aria-label={t("terminalWorkbench.resize")} aria-orientation={horizontal ? "vertical" : "horizontal"} aria-valuenow={Math.round(split.ratio * 100)} className="absolute z-20 hover:bg-primary/30 focus:bg-primary/30" style={style} onKeyDown={e => { if (["ArrowLeft", "ArrowUp", "ArrowRight", "ArrowDown"].includes(e.key)) { e.preventDefault(); setLayout(current => setSplitRatio(current, split.id, split.ratio + (["ArrowLeft", "ArrowUp"].includes(e.key) ? -.05 : .05))); } }} onPointerDown={e => { e.preventDefault(); e.currentTarget.setPointerCapture(e.pointerId); }} onPointerMove={e => { if (!(e.buttons & 1) || !surface.current) return; const box = surface.current.getBoundingClientRect(); const percent = horizontal ? (e.clientX - box.left) / box.width * 100 : (e.clientY - box.top) / box.height * 100; setLayout(current => setSplitRatio(current, split.id, (percent - (horizontal ? rect.x : rect.y)) / (horizontal ? rect.w : rect.h))); }} />; })}
    </div>
    <input className="hidden" ref={fileInput} type="file" multiple onChange={e => { const files = Array.from(e.target.files ?? []); e.target.value = ""; if (files.length) void handle?.sendFiles(files).catch(showError); }} />
    <input className="hidden" ref={castInput} type="file" accept=".cast,.jsonl" onChange={e => { const file = e.target.files?.[0]; e.target.value = ""; if (!file) return; if (file.size > 16 * 1024 * 1024) { showError(t("terminalWorkbench.recordLimit")); return; } void file.text().then(text => setCast(parseCast(text))).catch(showError); }} />
    <Dialog open={dialog !== null} onOpenChange={value => { if (!value) { setDialog(null); setPassword(""); setConfirmation(""); } }}><DialogContent className="max-w-xl"><DialogHeader><DialogTitle>{t(`terminalWorkbench.${dialog === "lock" && status?.locked ? "unlock" : dialog ?? "terminal"}`)}</DialogTitle></DialogHeader>
      {(dialog === "history" || dialog === "quick" || dialog === "highlight") && <TerminalPreferencesPanel mode={dialog} scope={scopeOf(tab.endpoint)} preferences={preferences} change={(value: TerminalPreferences) => setPreferences(value)} send={(command, execute) => { setDialog(null); requestInput(tab.id, command, "paste", execute); }} />}
      {dialog === "broadcast" && <div className="space-y-3"><DialogDescription>{t("terminalWorkbench.broadcastHint")}</DialogDescription>{tabs.filter(row => row.endpoint.kind === "ssh").map(row => <label key={row.id} className="flex items-center gap-2 text-sm"><Checkbox checked={broadcast.includes(row.id)} disabled={statuses[row.id]?.locked || statuses[row.id]?.state !== "running"} onCheckedChange={checked => setBroadcast(ids => checked === true ? [...ids, row.id] : ids.filter(id => id !== row.id))} /><span className="break-all">{label(row)}</span></label>)}<Button variant="outline" onClick={() => setBroadcast([])}>{t("terminalWorkbench.stopBroadcast")}</Button></div>}
      {dialog === "lock" && <div className="space-y-3"><Input type="password" autoComplete="off" maxLength={1024} value={password} onChange={e => setPassword(e.target.value)} placeholder={t("terminalWorkbench.password")} />{!status?.locked && <Input type="password" autoComplete="off" maxLength={1024} value={confirmation} onChange={e => setConfirmation(e.target.value)} placeholder={t("terminalWorkbench.confirmPassword")} />}<label className="flex items-center gap-2 text-sm"><Checkbox checked={lockAll} onCheckedChange={v => setLockAll(v === true)} />{t("terminalWorkbench.lockAll")}</label>{!status?.locked && <label className="flex items-center gap-2 text-sm"><Checkbox checked={hideOutput} onCheckedChange={v => setHideOutput(v === true)} />{t("terminalWorkbench.hideOutput")}</label>}<DialogFooter><Button onClick={() => void applyLock()} disabled={!password}>{t(`terminalWorkbench.${status?.locked ? "unlock" : "lock"}`)}</Button></DialogFooter></div>}
      {dialog === "integration" && <div className="space-y-3"><DialogDescription>{t("terminalWorkbench.integrationHint")}</DialogDescription><Select value={shell} onValueChange={value => setShell(value as IntegrationShell)}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent>{["bash", "zsh", "powershell"].map(value => <SelectItem key={value} value={value}>{value}</SelectItem>)}</SelectContent></Select><Button onClick={() => { setDialog(null); void handle?.integrate(shell).catch(showError); }}>{t("terminalWorkbench.enableIntegration")}</Button></div>}
      {dialog === "record" && <div className="space-y-3"><DialogDescription>{t("terminalWorkbench.recordHint")}</DialogDescription><Button onClick={() => { try { handle?.record(true); setDialog(null); } catch (e) { showError(e); } }}>{t("terminalWorkbench.startRecording")}</Button></div>}
      {dialog === "restoreLayout" && <div className="space-y-3"><DialogDescription>{t("terminalWorkbench.restoreHint")}</DialogDescription><Button onClick={restoreLayout}>{t("terminalWorkbench.confirm")}</Button></div>}
      {dialog === "close" && <div className="space-y-3"><DialogDescription>{t("terminalWorkbench.closeHint")}</DialogDescription><Button onClick={closeTab}>{t("terminalWorkbench.close")}</Button></div>}
    </DialogContent></Dialog>
    <Dialog open={!!decision} onOpenChange={value => { if (!value) setDecision(null); }}><DialogContent><DialogHeader><DialogTitle>{t("terminalWorkbench.confirmPaste")}</DialogTitle><DialogDescription>{t("terminalWorkbench.pasteHint")}</DialogDescription></DialogHeader><div className="max-h-28 overflow-auto text-xs">{decision?.targets.map(target => <p key={target.tab.id}>{label(target.tab)}</p>)}</div><pre className="max-h-48 overflow-auto whitespace-pre-wrap break-all rounded bg-ui-tint p-3 text-xs">{decision?.text.slice(0, 4096)}</pre>{decision?.reasons.map(reason => <p key={reason} className="text-xs text-amber-600">{t(`terminalWorkbench.pasteReasons.${reason}`)}</p>)}{decision?.execute && <p className="text-sm text-amber-600">{t("terminalWorkbench.willExecute")}</p>}<DialogFooter><Button variant="outline" onClick={() => setDecision(null)}>{t("terminalWorkbench.cancel")}</Button><Button onClick={() => { const input = decision; setDecision(null); if (input) void dispatch(input); }}>{t("terminalWorkbench.confirm")}</Button></DialogFooter></DialogContent></Dialog>
    <Dialog open={!!cast} onOpenChange={value => { if (!value) setCast(null); }}><DialogContent className="max-w-5xl"><DialogHeader><DialogTitle>{t("terminalWorkbench.replay")}</DialogTitle></DialogHeader>{cast && <TerminalPlayback cast={cast} />}</DialogContent></Dialog>
  </div>;
}
