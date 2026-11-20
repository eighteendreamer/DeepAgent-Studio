export type SplitDirection = "left" | "right" | "top" | "bottom";
export type TerminalPane = { kind: "pane"; id: string; tabs: string[]; active: string };
export type TerminalLayout = TerminalPane | { kind: "split"; id: string; axis: "horizontal" | "vertical"; ratio: number; first: TerminalLayout; second: TerminalLayout };
const id = () => crypto.randomUUID();
export function initialLayout(tab: string): TerminalLayout { return { kind: "pane", id: id(), tabs: [tab], active: tab }; }
export function collectPanes(layout: TerminalLayout): TerminalPane[] { return layout.kind === "pane" ? [layout] : [...collectPanes(layout.first), ...collectPanes(layout.second)]; }
export function updatePane(layout: TerminalLayout, pane: string, update: (p: TerminalPane) => TerminalPane): TerminalLayout {
  if (layout.kind === "pane") return layout.id === pane ? update(layout) : layout;
  return { ...layout, first: updatePane(layout.first, pane, update), second: updatePane(layout.second, pane, update) };
}
export function splitPane(layout: TerminalLayout, pane: string, direction: SplitDirection, tab: string): TerminalLayout {
  if (collectPanes(layout).length >= 8) throw new Error("A terminal workspace supports at most 8 panes");
  if (layout.kind !== "pane") return { ...layout, first: splitNode(layout.first), second: splitNode(layout.second) };
  return splitNode(layout);
  function splitNode(node: TerminalLayout): TerminalLayout {
    if (node.kind === "split") return { ...node, first: splitNode(node.first), second: splitNode(node.second) };
    if (node.id !== pane) return node;
    const next = initialLayout(tab); const before = direction === "left" || direction === "top";
    return { kind: "split", id: id(), axis: direction === "left" || direction === "right" ? "horizontal" : "vertical", ratio: 0.5, first: before ? next : node, second: before ? node : next };
  }
}
export function removeTab(layout: TerminalLayout, tab: string): TerminalLayout | null {
  if (layout.kind === "pane") {
    const tabs = layout.tabs.filter(t => t !== tab);
    return tabs.length ? { ...layout, tabs, active: tabs.includes(layout.active) ? layout.active : tabs[0] } : null;
  }
  const first = removeTab(layout.first, tab), second = removeTab(layout.second, tab);
  return first && second ? { ...layout, first, second } : first ?? second;
}
export function moveTab(layout: TerminalLayout, tab: string, destination: string): TerminalLayout {
  const panes = collectPanes(layout); const origin = panes.find(p => p.tabs.includes(tab));
  if (!origin || origin.id === destination || !panes.some(p => p.id === destination)) return layout;
  const next = removeTab(layout, tab)!;
  return updatePane(next, destination, pane => ({ ...pane, tabs: [...pane.tabs, tab], active: tab }));
}
export function setSplitRatio(layout: TerminalLayout, split: string, ratio: number): TerminalLayout {
  if (layout.kind === "pane") return layout;
  return { ...layout, ratio: layout.id === split ? Math.min(.85, Math.max(.15, ratio)) : layout.ratio, first: setSplitRatio(layout.first, split, ratio), second: setSplitRatio(layout.second, split, ratio) };
}
export function validateLayout(value: unknown): TerminalLayout | null {
  const ids = new Set<string>(), tabs = new Set<string>(); let count = 0;
  function read(v: unknown, depth: number): TerminalLayout {
    if (!v || typeof v !== "object" || depth > 8 || ++count > 15) throw new Error("layout limit");
    const n = v as TerminalLayout;
    if (typeof n.id !== "string" || n.id.length > 100 || ids.has(n.id)) throw new Error("invalid pane identity"); ids.add(n.id);
    if (n.kind === "split") {
      if (!["horizontal", "vertical"].includes(n.axis) || !Number.isFinite(n.ratio) || n.ratio < .15 || n.ratio > .85) throw new Error("invalid split");
      return { kind: n.kind, id: n.id, axis: n.axis, ratio: n.ratio, first: read(n.first, depth + 1), second: read(n.second, depth + 1) };
    }
    if (n.kind !== "pane" || !Array.isArray(n.tabs) || !n.tabs.length || n.tabs.length > 24 || !n.tabs.includes(n.active)) throw new Error("invalid tabs");
    for (const tab of n.tabs) { if (typeof tab !== "string" || tab.length > 100 || tabs.has(tab)) throw new Error("duplicate tab"); tabs.add(tab); }
    return { kind: n.kind, id: n.id, tabs: [...n.tabs], active: n.active };
  }
  try { return read(value, 0); } catch { return null; }
}
