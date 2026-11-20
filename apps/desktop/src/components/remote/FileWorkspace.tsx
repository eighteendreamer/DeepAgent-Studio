import { useEffect, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, ArrowUp, File, Folder, RefreshCw, Star, Terminal } from "lucide-react";
import { useTranslation } from "react-i18next";
import { sshListConnections, terminalCwd, type SshConnection } from "../../api";
import { filePanelList } from "../../fileWorkspaceApi";
import { writeClipboardText } from "../../clipboard";
import { observeRemoteTransfers } from "../../remoteTransferObserver";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Checkbox } from "../shadcn/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";
import { RemoteFileProperties } from "./RemoteFileProperties";
import { endpointKey, orderedEntries, panelStorageKey, stepDirectory, visitDirectory,
  type DirectoryHistory, type FileEndpoint, type FilePanelEntry, type FilePanelListing } from "./filePanelModel";

export interface PanelSelection { endpoint: FileEndpoint; cwd: string; selected: FilePanelEntry[] }
interface Props {
  connection: SshConnection;
  onOpenTerminal: (endpoint: FileEndpoint, cwd: string) => void;
  onOpenFile: (connectionId: string, path: string, name: string) => void;
  onCopy: (source: PanelSelection, destination: PanelSelection) => void;
  onExternalEdit: (connectionId: string, path: string) => void;
  onExtract: (connectionId: string, path: string, directory: string) => void;
  terminalDirectory?: { endpoint: FileEndpoint; path: string; token: number };
}

export function FileWorkspace(props: Props) {
  const { t } = useTranslation();
  const [connectionsError, setConnectionsError] = useState("");
  const [connections, setConnections] = useState<SshConnection[]>([props.connection]);
  const [leftEndpoint, setLeftEndpoint] = useState<FileEndpoint>({ kind: "local" });
  const [rightEndpoint, setRightEndpoint] = useState<FileEndpoint>({ kind: "ssh", connectionId: props.connection.id });
  const [left, setLeft] = useState<PanelSelection | null>(null);
  const [right, setRight] = useState<PanelSelection | null>(null);
  useEffect(() => { let active = true; void sshListConnections().then(list => { if (active) setConnections(list); }).catch(error => { if (active) setConnectionsError(String(error)); }); return () => { active = false; }; }, []);
  const panel = (side: "left" | "right", endpoint: FileEndpoint, setEndpoint: (endpoint: FileEndpoint) => void, onSelection: (selection: PanelSelection | null) => void) =>
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <EndpointPicker endpoint={endpoint} connections={connections} onChange={next => { setEndpoint(next); if (side === "left") setLeft(null); else setRight(null); }} />
      <FilePanel key={`${side}:${endpointKey(endpoint)}`} side={side} endpoint={endpoint} onSelection={onSelection} {...props} />
    </div>;
  return <div className="flex h-full min-h-0 flex-col" data-file-workspace>
    {connectionsError && <p role="alert" className="text-red-500">{connectionsError}</p>}
    <div className="flex min-h-0 flex-1 gap-1 p-2">
      {panel("left", leftEndpoint, setLeftEndpoint, setLeft)}
      <div className="flex shrink-0 flex-col justify-center gap-3 px-1">
        <Button size="icon" variant="outline" aria-label={t("remote.files.copyRight")} disabled={!left?.selected.length || !right} onClick={() => left && right && props.onCopy(left, right)}><ArrowRight className="h-4 w-4" /></Button>
        <Button size="icon" variant="outline" aria-label={t("remote.files.copyLeft")} disabled={!right?.selected.length || !left} onClick={() => right && left && props.onCopy(right, left)}><ArrowLeft className="h-4 w-4" /></Button>
      </div>
      {panel("right", rightEndpoint, setRightEndpoint, setRight)}
    </div>
  </div>;
}

function EndpointPicker({ endpoint, connections, onChange }: {
  endpoint: FileEndpoint; connections: SshConnection[]; onChange: (endpoint: FileEndpoint) => void;
}) {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");
  const current = endpointKey(endpoint);
  const matches = connections.filter(connection => connection.id === (endpoint.kind === "ssh" ? endpoint.connectionId : "") ||
    `${connection.name} ${connection.username}@${connection.host}`.toLocaleLowerCase().includes(search.toLocaleLowerCase()));
  return <div className="flex flex-wrap gap-1 pb-2">
    <Input className="h-8 min-w-0 flex-1" aria-label={t("remote.files.endpointSearch")} placeholder={t("remote.files.endpointSearch")} value={search} onChange={event => setSearch(event.target.value)} />
    <Select value={current} onValueChange={value => onChange(value === "local" ? { kind: "local" } : { kind: "ssh", connectionId: value.slice(4) })}>
      <SelectTrigger className="h-8 min-w-0 flex-1" aria-label={t("remote.files.endpoint")}><SelectValue /></SelectTrigger>
      <SelectContent><SelectItem value="local">{t("remote.files.local")}</SelectItem>
        {matches.map(connection => <SelectItem key={connection.id} value={`ssh:${connection.id}`}>{connection.name} · {connection.username}@{connection.host}:{connection.port}{connection.status === "connected" ? "" : ` · ${t("remote.notConnected")}`}</SelectItem>)}
      </SelectContent>
    </Select>
  </div>;
}

function restorePanel(key: string): { history: DirectoryHistory; favorites: string[]; sort: "name" | "size" | "modified" } {
  try {
    const value = JSON.parse(localStorage.getItem(key) ?? "null");
    if (Array.isArray(value?.history?.paths) && value.history.paths.length <= 100 && value.history.paths.every((path: unknown) => typeof path === "string" && path.length < 32768) &&
      Number.isInteger(value.history.index) && value.history.index >= 0 && value.history.index < value.history.paths.length) {
      return { history: value.history, favorites: Array.isArray(value.favorites) ? value.favorites.filter((path: unknown) => typeof path === "string" && path.length < 32768).slice(0, 50) : [], sort: ["name", "size", "modified"].includes(value.sort) ? value.sort : "name" };
    }
  } catch { /* Corrupt preferences cannot select another endpoint. */ }
  return { history: { paths: [], index: -1 }, favorites: [], sort: "name" };
}

function FilePanel({ side, endpoint, onSelection, onOpenTerminal, onOpenFile, onExternalEdit, onExtract, terminalDirectory }: Props & {
  side: string; endpoint: FileEndpoint; onSelection: (selection: PanelSelection | null) => void;
}) {
  const { t } = useTranslation();
  const key = panelStorageKey(side, endpoint);
  const [saved] = useState(() => restorePanel(key));
  const [history, setHistory] = useState(saved.history);
  const historyRef = useRef(history); historyRef.current = history;
  const [listing, setListing] = useState<FilePanelListing | null>(null);
  const listingRef = useRef(listing); listingRef.current = listing;
  const [address, setAddress] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [favorites, setFavorites] = useState(saved.favorites);
  const [sort, setSort] = useState(saved.sort);
  const [search, setSearch] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const loading = useRef(false);
  const [properties, setProperties] = useState<string | null>(null);
  const generation = useRef(0);
  const load = async (path: string, nextHistory?: DirectoryHistory) => {
    const owner = ++generation.current;
    loading.current = true;
    setBusy(true); setError("");
    try {
      const result = await filePanelList(endpoint, path);
      if (owner !== generation.current) return;
      if (endpointKey(result.endpoint) !== endpointKey(endpoint)) throw new Error("Unexpected endpoint identity");
      setListing(result); setAddress(result.path); setSelected([]);
      setHistory(nextHistory ? { ...nextHistory, paths: nextHistory.paths.map((value, index) => index === nextHistory.index ? result.path : value) } : visitDirectory(historyRef.current, result.path));
    } catch (error) { if (owner === generation.current) setError(String(error)); }
    finally { if (owner === generation.current) { loading.current = false; setBusy(false); } }
  };
  useEffect(() => {
    let active = true;
    void (async () => {
      const initial = saved.history.paths[saved.history.index] ?? (endpoint.kind === "ssh" ? "." : await terminalCwd());
      if (active) await load(initial);
    })().catch(error => { if (active) setError(String(error)); });
    return () => { active = false; generation.current++; };
  }, []);
  useEffect(() => {
    try { localStorage.setItem(key, JSON.stringify({ history, favorites, sort })); } catch { /* Browsing works when preferences are unavailable. */ }
  }, [key, history, favorites, sort]);
  useEffect(() => {
    onSelection(listing && !busy ? { endpoint, cwd: listing.path, selected: listing.entries.filter(entry => selected.includes(entry.path)) } : null);
  }, [listing, selected, busy]);
  useEffect(() => {
    let seen = "";
    return observeRemoteTransfers({ next: snapshot => {
      const terminal = snapshot.tasks.filter(task => ["completed", "failed", "cancelled"].includes(task.state)).map(task => `${task.id}:${task.attempt}:${task.state}`).join(",");
      const revision = terminal || "initial";
      if (seen && seen !== revision && !loading.current && listingRef.current) void load(listingRef.current.path, historyRef.current);
      seen = revision;
    }, error: error => setError(String(error)) });
  }, []);
  const entries = orderedEntries(listing?.entries ?? [], search, sort);
  const selectedEntry = listing?.entries.find(entry => entry.path === selected[0]);
  const terminalPath = busy ? undefined : listing?.path;
  const act = (work: Promise<unknown>) => { void work.catch(error => setError(String(error))); };
  const step = (delta: number) => { const next = stepDirectory(history, delta); const path = next.paths[next.index]; if (path) void load(path, next); };
  return <section className="flex min-h-0 flex-1 flex-col rounded-lg border border-border-theme bg-bg-base" aria-label={`${t("remote.files.panel")} ${side}`}>
    <div className="flex items-center gap-1 p-1">
      <Button variant="ghost" size="icon" aria-label={t("remote.files.back")} disabled={busy || history.index <= 0} onClick={() => step(-1)}><ArrowLeft className="h-3.5 w-3.5" /></Button>
      <Button variant="ghost" size="icon" aria-label={t("remote.files.forward")} disabled={busy || history.index >= history.paths.length - 1} onClick={() => step(1)}><ArrowRight className="h-3.5 w-3.5" /></Button>
      <Button variant="ghost" size="icon" aria-label={t("remote.files.parent")} disabled={busy || !listing?.parent} onClick={() => listing?.parent && void load(listing.parent)}><ArrowUp className="h-3.5 w-3.5" /></Button>
      <form className="min-w-0 flex-1" onSubmit={event => { event.preventDefault(); void load(address); }}><Input className="h-8" aria-label={t("remote.files.path")} value={address} onChange={event => setAddress(event.target.value)} /></form>
      <Button variant="ghost" size="icon" aria-label={t("remote.treeRefresh")} disabled={busy} onClick={() => void load(listing?.path ?? address, history)}><RefreshCw className={`h-3.5 w-3.5 ${busy ? "animate-spin" : ""}`} /></Button>
      <Button variant="ghost" size="icon" aria-label={t("remote.files.favorite")} disabled={!listing} onClick={() => listing && setFavorites(previous => previous.includes(listing.path) ? previous.filter(path => path !== listing.path) : [...previous, listing.path].slice(-50))}><Star className={`h-3.5 w-3.5 ${listing && favorites.includes(listing.path) ? "fill-current" : ""}`} /></Button>
    </div>
    {favorites.length > 0 && <div className="flex shrink-0 gap-1 overflow-x-auto px-2 pb-1">{favorites.map(path => <Button key={path} className="h-6 shrink-0 text-[11px]" variant="ghost" size="sm" onClick={() => void load(path)}>{path}</Button>)}</div>}
    <div className="flex flex-wrap items-center gap-1 border-b border-border-theme px-2 pb-2">
      <Input className="h-7 min-w-[90px] flex-1" value={search} onChange={event => setSearch(event.target.value)} aria-label={t("remote.searchFiles")} placeholder={t("remote.searchFiles")} />
      <Select value={sort} onValueChange={value => setSort(value as typeof sort)}><SelectTrigger className="h-7 w-24" aria-label={t("remote.files.sort")}><SelectValue /></SelectTrigger><SelectContent>{["name", "size", "modified"].map(value => <SelectItem key={value} value={value}>{t(`remote.files.${value}`)}</SelectItem>)}</SelectContent></Select>
      <Button variant="ghost" size="sm" disabled={!terminalPath} onClick={() => terminalPath && onOpenTerminal(endpoint, terminalPath)}><Terminal className="mr-1 h-3 w-3" />{t("remote.files.openTerminal")}</Button>
      {terminalDirectory && endpointKey(terminalDirectory.endpoint) === endpointKey(endpoint) && <Button variant="ghost" size="sm" onClick={() => void load(terminalDirectory.path)}>{t("remote.files.followTerminal")}</Button>}
    </div>
    {error && <p role="alert" className="break-all px-2 py-1 text-[12px] text-red-500">{error}</p>}
    <div className="min-h-0 flex-1 overflow-auto p-1" aria-busy={busy}>
      {entries.map(entry => <div key={entry.path} className={`flex items-center gap-1 rounded px-1 text-[12px] ${selected.includes(entry.path) ? "bg-sidebar-highlight" : "hover:bg-ui-tint"}`}>
        <Checkbox aria-label={`${t("remote.files.select")} ${entry.name}`} checked={selected.includes(entry.path)} onCheckedChange={checked => setSelected(previous => checked ? [...previous, entry.path] : previous.filter(path => path !== entry.path))} />
        <Button variant="ghost" className="h-8 min-w-0 flex-1 justify-start gap-2 px-1 text-left text-[12px] font-normal" onClick={() => setSelected([entry.path])} onDoubleClick={() => entry.isDir && !entry.isSymlink ? void load(entry.path) : endpoint.kind === "ssh" && !entry.isSymlink && onOpenFile(endpoint.connectionId, entry.path, entry.name)}>
          {entry.isDir ? <Folder className="h-3.5 w-3.5 shrink-0" /> : <File className="h-3.5 w-3.5 shrink-0" />}<span className="min-w-0 flex-1 truncate">{entry.name}{entry.isSymlink ? " ↗" : ""}</span>
          {!entry.isDir && <span className="shrink-0 text-[10px] text-text-secondary">{entry.sizeBytes ?? "?"} B</span>}
        </Button>
      </div>)}
      {!busy && !entries.length && <p className="p-3 text-[12px] text-text-secondary">{t("remote.noMatch")}</p>}
    </div>
    <div className="flex flex-wrap gap-1 border-t border-border-theme p-1 text-[11px]">
      <Button variant="ghost" size="sm" disabled={!selectedEntry} onClick={() => selectedEntry && act(writeClipboardText(selectedEntry.name))}>{t("remote.files.copyName")}</Button>
      <Button variant="ghost" size="sm" disabled={!selectedEntry} onClick={() => selectedEntry && act(writeClipboardText(selectedEntry.path))}>{t("remote.files.copyPath")}</Button>
      {endpoint.kind === "ssh" && <>
        <Button variant="ghost" size="sm" disabled={!selectedEntry} onClick={() => selectedEntry && setProperties(selectedEntry.path)}>{t("remote.properties.title")}</Button>
        <Button variant="ghost" size="sm" disabled={!selectedEntry || selectedEntry.isDir || selectedEntry.isSymlink} onClick={() => selectedEntry && onExternalEdit(endpoint.connectionId, selectedEntry.path)}>{t("remote.files.externalEdit")}</Button>
        <Button variant="ghost" size="sm" disabled={!selectedEntry || selectedEntry.isDir || selectedEntry.isSymlink} onClick={() => selectedEntry && listing && onExtract(endpoint.connectionId, selectedEntry.path, listing.path)}>{t("remote.files.extract")}</Button>
      </>}
      <span className="p-2 text-text-secondary">{selected.length} / {listing?.entries.length ?? 0}</span>
    </div>
    {properties && endpoint.kind === "ssh" && <RemoteFileProperties key={properties} connectionId={endpoint.connectionId} path={properties} onClose={() => setProperties(null)} />}
  </section>;
}
