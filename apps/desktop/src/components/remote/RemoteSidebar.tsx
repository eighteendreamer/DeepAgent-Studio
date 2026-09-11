import {
  ArrowLeft,
  ChevronDown,
  ChevronRight,
  File as FileIcon,
  Folder,
  FolderOpen,
  Loader2,
  Plus,
  Search,
  Server,
} from "lucide-react";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { message } from "../message";
import { normalizeSshError } from "../settings/ConnectionsSettings";
import {
  pickSshIdentityFile,
  sshConnect,
  sshCreateConnection,
  sshListConnections,
  sshListDir,
  type SshConnection,
  type SshDirEntry,
} from "../../api";

type AuthType = "password" | "file";

type ManagerMode = "list" | "create";

interface RemoteSidebarProps {
  selected: SshConnection | null;
  onSelect: (conn: SshConnection) => void;
  onOpenFile: (path: string, name: string) => void;
  onBack: () => void;
}

interface ConnectionForm {
  name: string;
  host: string;
  port: string;
  username: string;
  authType: AuthType;
  keyPath: string;
  password: string;
}

const emptyForm: ConnectionForm = {
  name: "",
  host: "",
  port: "22",
  username: "",
  authType: "password",
  keyPath: "",
  password: "",
};

const statusDotClassName = (status: SshConnection["status"]) => {
  switch (status) {
    case "connected":
      return "bg-green-500";
    case "connecting":
      return "bg-yellow-400";
    case "error":
      return "bg-red-400";
    default:
      return "bg-gray-300";
  }
};

export function RemoteSidebar({
  selected,
  onSelect,
  onOpenFile,
  onBack,
}: RemoteSidebarProps) {
  const { t } = useTranslation();
  const [connections, setConnections] = useState<SshConnection[]>([]);
  const [connectingId, setConnectingId] = useState<string | null>(null);
  const [managerOpen, setManagerOpen] = useState(false);
  const [managerMode, setManagerMode] = useState<ManagerMode>("list");
  const [form, setForm] = useState<ConnectionForm>(emptyForm);
  const [filter, setFilter] = useState("");
  const [tree, setTree] = useState<Record<string, SshDirEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [loadingDir, setLoadingDir] = useState<string | null>(null);
  const [dirError, setDirError] = useState<string | null>(null);

  const loadConnections = () => {
    sshListConnections()
      .then(setConnections)
      .catch(() => setConnections([]));
  };

  useEffect(() => {
    loadConnections();
  }, []);

  const loadDir = async (connectionId: string, path: string) => {
    setLoadingDir(path);
    try {
      const listing = await sshListDir(connectionId, path);
      setTree((prev) => ({ ...prev, [path]: listing.entries }));
      setDirError(null);
    } catch (error) {
      setDirError(
        normalizeSshError(
          error instanceof Error ? error.message : String(error),
          t,
        ),
      );
    } finally {
      setLoadingDir(null);
    }
  };

  const handleSelect = async (conn: SshConnection) => {
    setManagerOpen(false);
    onSelect(conn);
    setTree({});
    setExpanded(new Set());
    setDirError(null);
    setFilter("");
    let current = conn;
    if (conn.status !== "connected") {
      setConnectingId(conn.id);
      try {
        await sshConnect(conn.id);
      } catch (error) {
        const displayError = normalizeSshError(
          error instanceof Error ? error.message : String(error),
          t,
        );
        message.error(`${t("remote.connectFailed")}: ${displayError}`);
      } finally {
        setConnectingId(null);
      }
      const list = await sshListConnections().catch(() => []);
      setConnections(list);
      const refreshed = list.find((item) => item.id === conn.id);
      if (refreshed) {
        current = refreshed;
        onSelect(refreshed);
      }
      if (current.status !== "connected") return;
    }
    await loadDir(current.id, "/");
  };

  const toggleDir = (entry: SshDirEntry) => {
    const next = new Set(expanded);
    if (next.has(entry.path)) {
      next.delete(entry.path);
      setExpanded(next);
      return;
    }
    next.add(entry.path);
    setExpanded(next);
    if (selected && !tree[entry.path]) void loadDir(selected.id, entry.path);
  };

  const handlePickKeyFile = async () => {
    const selectedPath = await pickSshIdentityFile();
    if (!selectedPath) return;
    setForm((prev) => ({ ...prev, keyPath: selectedPath }));
  };

  const handleSave = async () => {
    if (!form.name || !form.host || !form.username) return;
    try {
      const created = await sshCreateConnection(
        form.name,
        form.host,
        parseInt(form.port, 10) || 22,
        form.username,
        form.authType === "file" ? "key_file" : "password",
        form.authType === "file" ? form.keyPath : undefined,
        form.authType === "password" ? form.password : undefined,
      );
      setForm(emptyForm);
      setManagerMode("list");
      loadConnections();
      void handleSelect(created);
    } catch (error) {
      const displayError = normalizeSshError(
        error instanceof Error ? error.message : String(error),
        t,
      );
      message.error(`${t("remote.createFailed")}: ${displayError}`);
    }
  };

  const statusLabel = (status: SshConnection["status"]) => {
    switch (status) {
      case "connected":
        return t("settings.connections.online");
      case "connecting":
        return t("settings.connections.checking");
      default:
        return t("settings.connections.offline");
    }
  };

  const renderRows = (entries: SshDirEntry[], depth: number) => (
    <>
      {entries.map((entry) => {
        const rowClassName =
          "flex w-full items-center gap-1 rounded-md py-1 pr-1.5 text-left text-[12px] text-text-base hover:bg-black/5";
        if (entry.is_dir) {
          const isOpen = expanded.has(entry.path);
          const children = tree[entry.path];
          return (
            <div key={entry.path}>
              <button
                type="button"
                className={rowClassName}
                style={{ paddingLeft: 4 + depth * 12 }}
                title={entry.path}
                onClick={() => toggleDir(entry)}
              >
                {isOpen ? (
                  <ChevronDown className="h-3 w-3 shrink-0 text-text-secondary" />
                ) : (
                  <ChevronRight className="h-3 w-3 shrink-0 text-text-secondary" />
                )}
                {isOpen ? (
                  <FolderOpen className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
                ) : (
                  <Folder className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
                )}
                <span className="truncate">{entry.name}</span>
                {loadingDir === entry.path && (
                  <Loader2 className="h-3 w-3 shrink-0 animate-spin text-text-secondary" />
                )}
              </button>
              {isOpen && children && renderRows(children, depth + 1)}
            </div>
          );
        }
        return (
          <button
            key={entry.path}
            type="button"
            className="flex w-full items-center gap-1 rounded-md py-1 pr-1.5 pl-0 text-left text-[12px] text-text-base hover:bg-black/5"
            style={{ paddingLeft: 4 + depth * 12 }}
            title={entry.path}
            onClick={() => onOpenFile(entry.path, entry.name)}
          >
            <span className="w-3 shrink-0" />
            <FileIcon className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
            <span className="truncate">{entry.name}</span>
          </button>
        );
      })}
    </>
  );

  const rootEntries = tree["/"];
  const query = filter.trim().toLowerCase();
  const searchMatches = query
    ? Object.entries(tree).flatMap(([dir, entries]) =>
        entries
          .filter((entry) => entry.name.toLowerCase().includes(query))
          .map((entry) => ({ dir, entry })),
      )
    : [];

  const connectionRows = (
    <div className="space-y-0.5">
      {connections.length === 0 ? (
        <div className="px-2.5 py-1 text-[12px] text-text-secondary">
          {t("remote.noConnections")}
        </div>
      ) : (
        connections.map((conn) => (
          <button
            key={conn.id}
            type="button"
            disabled={connectingId === conn.id}
            onClick={() => void handleSelect(conn)}
            className={`flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left transition-colors hover:bg-black/5 disabled:opacity-60 ${
              selected?.id === conn.id ? "bg-sidebar-highlight" : ""
            }`}
          >
            <Server className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
            <span className="min-w-0 flex-1">
              <span className="block truncate text-[12px] text-text-base">
                {conn.name}
              </span>
              <span className="block truncate text-[10px] text-text-secondary">
                {conn.username}@{conn.host}:{conn.port}
              </span>
            </span>
            {connectingId === conn.id ? (
              <Loader2 className="h-3 w-3 shrink-0 animate-spin text-text-secondary" />
            ) : (
              <span
                className={`h-1.5 w-1.5 shrink-0 rounded-full ${statusDotClassName(conn.status)}`}
                title={statusLabel(conn.status)}
              />
            )}
          </button>
        ))
      )}
    </div>
  );

  return (
    <aside className="flex h-full w-[240px] flex-shrink-0 flex-col bg-sidebar-bg no-select pb-2">
      <div className="px-3 pt-4 pb-3">
        <button
          type="button"
          onClick={onBack}
          className="flex items-center px-2 text-[13px] font-medium text-text-secondary transition-colors hover:text-text-base"
        >
          <ArrowLeft className="mr-2 h-4 w-4" />
          {t("remote.back")}
        </button>
      </div>

      <div className="px-3 pb-2">
        <button
          type="button"
          onClick={() => {
            setManagerMode("list");
            setManagerOpen(true);
          }}
          className="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-[13px] font-medium text-text-base transition-colors hover:bg-black/5"
        >
          <Server className="h-4 w-4 shrink-0 text-text-secondary" />
          <span className="min-w-0 flex-1 truncate text-left">
            {selected ? selected.name : t("remote.selectConnection")}
          </span>
          {connectingId === selected?.id ? (
            <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-text-secondary" />
          ) : (
            <ChevronDown className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
          )}
        </button>
      </div>

      <div className="flex-1 overflow-y-auto px-3">
        <div className="mb-1 flex items-center gap-1.5 px-2.5 text-[11px] font-medium text-text-secondary">
          <Search className="h-3 w-3" />
          {t("remote.searchFiles")}
        </div>
        <input
          type="text"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
          placeholder={t("remote.searchPlaceholder")}
          className="mb-2 h-[28px] w-full rounded-[10px] border border-border-theme bg-white px-2.5 text-[12px] text-text-base outline-none transition-colors focus:border-blue-500"
        />

        {dirError && (
          <div className="mb-2 rounded-md bg-red-50 px-2.5 py-1.5 text-[11px] text-red-500">
            {t("remote.loadFailed")}: {dirError}
          </div>
        )}

        <div className="text-[12px]">
          {!selected ? (
            <div className="px-2.5 py-1 text-text-secondary">
              {t("remote.emptyTree")}
            </div>
          ) : query ? (
            searchMatches.length === 0 ? (
              <div className="px-2.5 py-1 text-text-secondary">
                {t("remote.noMatch")}
              </div>
            ) : (
              searchMatches.map(({ dir, entry }) =>
                entry.is_dir ? (
                  <button
                    key={`${dir}/${entry.name}`}
                    type="button"
                    className="flex w-full items-center gap-1.5 rounded-md px-2.5 py-1 text-left hover:bg-black/5"
                    title={`${dir} · ${entry.path}`}
                    onClick={() => toggleDir(entry)}
                  >
                    <Folder className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
                    <span className="min-w-0">
                      <span className="block truncate text-[12px] text-text-base">
                        {entry.name}
                      </span>
                      <span className="block truncate text-[10px] text-text-secondary">
                        {dir}
                      </span>
                    </span>
                  </button>
                ) : (
                  <button
                    key={`${dir}/${entry.name}`}
                    type="button"
                    className="flex w-full items-center gap-1.5 rounded-md px-2.5 py-1 text-left hover:bg-black/5"
                    title={`${dir} · ${entry.path}`}
                    onClick={() => onOpenFile(entry.path, entry.name)}
                  >
                    <FileIcon className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
                    <span className="min-w-0">
                      <span className="block truncate text-[12px] text-text-base">
                        {entry.name}
                      </span>
                      <span className="block truncate text-[10px] text-text-secondary">
                        {dir}
                      </span>
                    </span>
                  </button>
                ),
              )
            )
          ) : rootEntries === undefined ? (
            <div className="flex items-center gap-1.5 px-2.5 py-1 text-text-secondary">
              <Loader2 className="h-3 w-3 animate-spin" />
              {t("settings.connections.checking")}
            </div>
          ) : (
            renderRows(rootEntries, 0)
          )}
        </div>
      </div>

      {managerOpen &&
        createPortal(
          <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-[rgba(15,23,42,0.16)] px-6 py-8"
            onMouseDown={() => setManagerOpen(false)}
          >
            <div
              className="flex max-h-[calc(100vh-64px)] w-full max-w-[520px] flex-col overflow-hidden rounded-[20px] bg-elevated-bg shadow-[0_6px_24px_rgba(0,0,0,0.10)]"
              onMouseDown={(event) => event.stopPropagation()}
            >
              {managerMode === "list" ? (
                <>
                  <div className="flex items-start justify-between gap-3 border-b border-border-theme px-5 py-4">
                    <h3 className="text-[18px] font-semibold tracking-tight text-text-base">
                      {t("remote.selectConnection")}
                    </h3>
                    <button
                      type="button"
                      className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-gray-400 transition-colors hover:bg-black/5 hover:text-text-base"
                      onClick={() => setManagerOpen(false)}
                    >
                      ×
                    </button>
                  </div>
                  <div className="min-h-0 flex-1 overflow-y-auto px-5 py-3">
                    {connectionRows}
                  </div>
                  <div className="flex justify-start border-t border-border-theme px-5 py-3">
                    <button
                      type="button"
                      onClick={() => {
                        setForm(emptyForm);
                        setManagerMode("create");
                      }}
                      className="flex items-center gap-1.5 rounded-full bg-black/5 px-4 py-1.5 text-[13px] text-text-base transition-colors hover:bg-black/10"
                    >
                      <Plus className="h-3.5 w-3.5" />
                      {t("remote.newConnection")}
                    </button>
                  </div>
                </>
              ) : (
                <>
                  <div className="flex items-start justify-between gap-3 border-b border-border-theme px-5 py-4">
                    <h3 className="text-[18px] font-semibold tracking-tight text-text-base">
                      {t("remote.newConnection")}
                    </h3>
                    <button
                      type="button"
                      className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-gray-400 transition-colors hover:bg-black/5 hover:text-text-base"
                      onClick={() => setManagerOpen(false)}
                    >
                      ×
                    </button>
                  </div>
                  <form
                    className="min-h-0 flex-1 overflow-y-auto"
                    onSubmit={(event) => {
                      event.preventDefault();
                      void handleSave();
                    }}
                  >
                    <div className="space-y-3 px-5 py-4">
                      <div>
                        <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                          {t("settings.connections.displayName")}
                        </label>
                        <input
                          type="text"
                          value={form.name}
                          onChange={(event) =>
                            setForm({ ...form, name: event.target.value })
                          }
                          className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                        />
                      </div>
                      <div>
                        <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                          {t("settings.connections.hostname")}
                        </label>
                        <input
                          type="text"
                          value={form.host}
                          onChange={(event) =>
                            setForm({ ...form, host: event.target.value })
                          }
                          placeholder={t("settings.connections.hostPlaceholder")}
                          className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                        />
                      </div>
                      <div className="grid gap-3 sm:grid-cols-[110px_minmax(0,1fr)]">
                        <div>
                          <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                            {t("settings.connections.sshPort")}{" "}
                            <span className="font-normal text-gray-400">
                              {t("settings.connections.optional")}
                            </span>
                          </label>
                          <input
                            type="text"
                            value={form.port}
                            onChange={(event) =>
                              setForm({ ...form, port: event.target.value })
                            }
                            className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                          />
                        </div>
                        <div>
                          <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                            {t("settings.connections.username")}
                          </label>
                          <input
                            type="text"
                            value={form.username}
                            onChange={(event) =>
                              setForm({ ...form, username: event.target.value })
                            }
                            className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                          />
                        </div>
                      </div>

                      <section className="space-y-2.5 border-t border-border-theme pt-3">
                        <div className="flex items-center gap-4 text-[12px]">
                          <label className="flex items-center gap-1.5">
                            <input
                              type="radio"
                              name="remote-auth"
                              checked={form.authType === "password"}
                              onChange={() =>
                                setForm({ ...form, authType: "password" })
                              }
                            />
                            {t("settings.connections.password")}
                          </label>
                          <label className="flex items-center gap-1.5">
                            <input
                              type="radio"
                              name="remote-auth"
                              checked={form.authType === "file"}
                              onChange={() =>
                                setForm({ ...form, authType: "file" })
                              }
                            />
                            {t("settings.connections.identityFile")}
                          </label>
                        </div>

                        {form.authType === "password" ? (
                          <div>
                            <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                              {t("settings.connections.password")}
                            </label>
                            <input
                              type="password"
                              value={form.password}
                              onChange={(event) =>
                                setForm({ ...form, password: event.target.value })
                              }
                              className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                            />
                          </div>
                        ) : (
                          <div>
                            <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                              {t("settings.connections.identityFilePath")}
                            </label>
                            <div className="flex gap-2">
                              <input
                                type="text"
                                value={form.keyPath}
                                onChange={(event) =>
                                  setForm({ ...form, keyPath: event.target.value })
                                }
                                className="h-[34px] min-w-0 flex-1 rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                              />
                              <button
                                type="button"
                                onClick={() => void handlePickKeyFile()}
                                className="h-[34px] shrink-0 rounded-[14px] bg-black/5 px-3 text-[12px] text-text-base transition-colors hover:bg-black/10"
                              >
                                {t("settings.connections.browse")}
                              </button>
                            </div>
                          </div>
                        )}
                      </section>
                    </div>
                    <div className="flex justify-between border-t border-border-theme px-5 py-3">
                      <button
                        type="button"
                        onClick={() => setManagerMode("list")}
                        className="rounded-full bg-black/5 px-4 py-1.5 text-[13px] text-text-base transition-colors hover:bg-black/10"
                      >
                        {t("remote.backToList")}
                      </button>
                      <button
                        type="submit"
                        disabled={!form.name || !form.host || !form.username}
                        className="rounded-full bg-black px-4 py-1.5 text-[13px] text-white transition-colors hover:bg-gray-800 disabled:opacity-60"
                      >
                        {t("settings.connections.save")}
                      </button>
                    </div>
                  </form>
                </>
              )}
            </div>
          </div>,
          document.body,
        )}
    </aside>
  );
}
