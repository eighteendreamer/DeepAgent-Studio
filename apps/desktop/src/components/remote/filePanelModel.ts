export type FileEndpoint = { kind: "local" } | { kind: "ssh"; connectionId: string };
export interface FilePanelEntry {
  name: string; path: string; isDir: boolean; isSymlink: boolean; sizeBytes: string | null; modifiedMs: number | null;
}
export interface FilePanelListing {
  endpoint: FileEndpoint; path: string; parent: string | null; entries: FilePanelEntry[];
}
export interface DirectoryHistory { paths: string[]; index: number }

export function endpointKey(endpoint: FileEndpoint): string {
  return endpoint.kind === "local" ? "local" : `ssh:${endpoint.connectionId}`;
}

export function panelStorageKey(side: string, endpoint: FileEndpoint): string {
  return `remote-file-panel:v1:${side}:${endpointKey(endpoint)}`;
}

export function visitDirectory(history: DirectoryHistory, path: string): DirectoryHistory {
  if (history.paths[history.index] === path) return history;
  const paths = [...history.paths.slice(0, history.index + 1), path].slice(-100);
  return { paths, index: paths.length - 1 };
}

export function stepDirectory(history: DirectoryHistory, delta: number): DirectoryHistory {
  const index = Math.max(0, Math.min(history.paths.length - 1, history.index + delta));
  return { ...history, index };
}

export function joinEndpointPath(endpoint: FileEndpoint, directory: string, name: string): string {
  if (!name || name === "." || name === ".." || name.includes("/") || name.includes("\0") ||
      (endpoint.kind === "local" && /[\\:]/.test(name))) throw new Error("Invalid destination name");
  const separator = endpoint.kind === "ssh" || !directory.includes("\\") ? "/" : "\\";
  return `${directory.replace(/[\\/]$/, "")}${separator}${name}`;
}

export function orderedEntries(entries: FilePanelEntry[], query: string, sort: "name" | "size" | "modified"): FilePanelEntry[] {
  const filtered = entries.filter(entry => entry.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  return filtered.sort((left, right) => Number(right.isDir) - Number(left.isDir) ||
    (sort === "size" ? compareSizes(left.sizeBytes, right.sizeBytes) :
      sort === "modified" ? (right.modifiedMs ?? 0) - (left.modifiedMs ?? 0) : 0) ||
    left.name.localeCompare(right.name));
}

function compareSizes(left: string | null, right: string | null): number {
  const a = BigInt(left ?? "0"), b = BigInt(right ?? "0");
  return a < b ? 1 : a > b ? -1 : 0;
}
