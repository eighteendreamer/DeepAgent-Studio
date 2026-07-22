import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { McpServer, McpConnectionStatus } from "../../types";
import {
  listMcpServers,
  saveMcpServer,
  removeMcpServer,
  setMcpServerEnabled,
  testMcpServer,
  mcpConnectionStatus,
} from "../../api";

function StatusBadge({ status }: { status: McpConnectionStatus["status"] }) {
  const { t } = useTranslation();
  const cfg: Record<McpConnectionStatus["status"], { cls: string; icon: IconProp; label: string }> = {
    connected: {
      cls: "bg-green-100 text-green-700",
      icon: ["fas", "circle-check"],
      label: t("settings.mcp.statusConnected"),
    },
    failed: {
      cls: "bg-red-100 text-red-600",
      icon: ["fas", "circle-exclamation"],
      label: t("settings.mcp.statusFailed"),
    },
    disabled: {
      cls: "bg-gray-100 text-gray-500",
      icon: ["fas", "minus"],
      label: t("settings.mcp.statusDisabled"),
    },
  };
  const c = cfg[status];
  return (
    <span className={`inline-flex items-center rounded-full px-2 py-0.5 text-[11px] font-medium ${c.cls}`}>
      <FontAwesomeIcon icon={c.icon} className="mr-1 text-[10px]" />
      {c.label}
    </span>
  );
}

function ToggleSwitch({ checked, onChange }: { checked: boolean; onChange: () => void }) {
  return (
    <div
      className={`w-9 h-5 rounded-full relative cursor-pointer transition-colors ${
        checked ? "bg-blue-500" : "bg-gray-300"
      }`}
      onClick={onChange}
    >
      <div
        className={`w-3.5 h-3.5 rounded-full bg-white absolute top-[3px] transition-transform ${
          checked ? "translate-x-[20px]" : "translate-x-[3px]"
        }`}
      />
    </div>
  );
}

function emptyDraft(): McpServer {
  return {
    name: "",
    transport: "stdio",
    enabled: true,
    command: "",
    args: [],
    env: {},
    url: "",
    headers: {},
    source: "user",
    source_plugin_id: null,
    source_plugin_name: null,
    declared_name: null,
    source_path: null,
    read_only: false,
    conflict: null,
  };
}

function sourceLabel(server: McpServer): string {
  if (server.source !== "plugin") return "用户配置";
  return `插件：${server.source_plugin_name || server.source_plugin_id || "unknown"}`;
}

export function MCPSettings() {
  const { t } = useTranslation();
  const [view, setView] = useState<"list" | "add">("list");
  const [servers, setServers] = useState<McpServer[]>([]);
  const [draft, setDraft] = useState<McpServer>(emptyDraft());
  const [error, setError] = useState<string | null>(null);
  const [envPairs, setEnvPairs] = useState<{ key: string; value: string }[]>([]);
  const [statuses, setStatuses] = useState<Record<string, McpConnectionStatus>>({});
  const [checking, setChecking] = useState(false);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  // Add-view "test connection" state.
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<McpConnectionStatus | null>(null);

  async function refresh() {
    try {
      setServers(await listMcpServers());
    } catch {
      setServers([]);
    }
  }

  async function refreshStatuses() {
    setChecking(true);
    try {
      const list = await mcpConnectionStatus();
      const map: Record<string, McpConnectionStatus> = {};
      for (const s of list) map[s.name] = s;
      setStatuses(map);
    } catch {
      setStatuses({});
    } finally {
      setChecking(false);
    }
  }

  useEffect(() => {
    refresh();
    refreshStatuses();
  }, []);

  function toggleExpanded(name: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  }

  function openAdd(existing?: McpServer) {
    if (existing?.read_only) return;
    const d = existing ? { ...existing } : emptyDraft();
    setDraft(d);
    setEnvPairs(Object.entries(d.env).map(([key, value]) => ({ key, value })));
    setError(null);
    setTestResult(null);
    setView("add");
  }

  async function onToggle(s: McpServer) {
    if (s.read_only) return;
    await setMcpServerEnabled(s.name, !s.enabled).catch(() => {});
    await refresh();
    refreshStatuses();
  }

  async function onRemove(server: McpServer) {
    if (server.read_only) return;
    const name = server.name;
    await removeMcpServer(name).catch(() => {});
    await refresh();
    refreshStatuses();
  }

  function draftPayload(): McpServer {
    const env: Record<string, string> = {};
    for (const { key, value } of envPairs) {
      if (key.trim()) env[key.trim()] = value;
    }
    return {
      ...draft,
      source: "user",
      source_plugin_id: null,
      source_plugin_name: null,
      declared_name: null,
      source_path: null,
      read_only: false,
      conflict: null,
      env,
      args: draft.args.filter((a) => a.trim() !== ""),
      command: draft.transport === "stdio" ? draft.command : null,
      url: draft.transport !== "stdio" ? draft.url : null,
    };
  }

  async function onTest() {
    setTesting(true);
    setTestResult(null);
    try {
      setTestResult(await testMcpServer(draftPayload()));
    } catch (e) {
      setTestResult({ name: draft.name, status: "failed", error: String(e), tools: [] });
    } finally {
      setTesting(false);
    }
  }

  async function onSave() {
    setError(null);
    try {
      await saveMcpServer(draftPayload());
      await refresh();
      refreshStatuses();
      setView("list");
    } catch (e) {
      setError(String(e));
    }
  }

  if (view === "add") {
    const isStdio = draft.transport === "stdio";
    const canSave =
      draft.name.trim() !== "" &&
      (isStdio ? (draft.command ?? "").trim() !== "" : (draft.url ?? "").trim() !== "");
    return (
      <div className="max-w-[700px]">
        <button
          className="flex items-center text-[13px] text-text-secondary hover:text-text-base transition-colors mb-6"
          onClick={() => setView("list")}
        >
          <FontAwesomeIcon icon={["fas", "arrow-left"]} className="mr-2 text-[12px]" /> {t("settings.mcp.back")}
        </button>

        <div className="mb-8">
          <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.mcp.connectCustom")}</h1>
          <div className="text-[13px] text-text-secondary">{t("settings.mcp.connectCustomDesc")}</div>
        </div>

        <div className="space-y-6">
          {/* 名称 */}
          <div>
            <div className="text-[13px] font-medium text-text-base mb-2">{t("settings.mcp.name")}</div>
            <input
              type="text"
              placeholder="MCP server name"
              value={draft.name}
              onChange={(e) => setDraft({ ...draft, name: e.target.value })}
              className="w-full border border-border-theme rounded-lg py-2 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white"
            />
          </div>

          {/* 传输协议 */}
          <div className="flex border border-border-theme rounded-lg overflow-hidden bg-white">
            <button
              className={`flex-1 py-2 text-[13px] font-medium transition-colors ${
                isStdio ? "bg-gray-200 text-text-base" : "text-text-secondary hover:bg-gray-50"
              }`}
              onClick={() => setDraft({ ...draft, transport: "stdio" })}
            >
              STDIO
            </button>
            <div className="w-[1px] bg-border-theme" />
            <button
              className={`flex-1 py-2 text-[13px] font-medium transition-colors ${
                draft.transport === "http"
                  ? "bg-gray-200 text-text-base"
                  : "text-text-secondary hover:bg-gray-50"
              }`}
              onClick={() => setDraft({ ...draft, transport: "http" })}
            >
              {t("settings.mcp.streamingHttp")}
            </button>
          </div>

          {isStdio ? (
            <>
              {/* 启动命令 */}
              <div>
                <div className="text-[13px] font-medium text-text-base mb-2">{t("settings.mcp.startCommand")}</div>
                <input
                  type="text"
                  placeholder="npx -y server-filesystem"
                  value={draft.command ?? ""}
                  onChange={(e) => setDraft({ ...draft, command: e.target.value })}
                  className="w-full border border-border-theme rounded-lg py-2 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white"
                />
              </div>

              {/* 参数 */}
              <div>
                <div className="text-[13px] font-medium text-text-base mb-2">{t("settings.mcp.args")}</div>
                {draft.args.map((arg, i) => (
                  <div key={i} className="flex items-center space-x-2 mb-2">
                    <input
                      type="text"
                      value={arg}
                      onChange={(e) => {
                        const args = [...draft.args];
                        args[i] = e.target.value;
                        setDraft({ ...draft, args });
                      }}
                      className="flex-1 border border-border-theme rounded-lg py-2 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white"
                    />
                    <button
                      className="text-gray-400 hover:text-red-500 transition-colors px-2"
                      onClick={() =>
                        setDraft({ ...draft, args: draft.args.filter((_, j) => j !== i) })
                      }
                    >
                      <FontAwesomeIcon icon={["fas", "minus"]} className="text-[12px]" />
                    </button>
                  </div>
                ))}
                <button
                  className="w-full py-1.5 bg-gray-100 hover:bg-gray-200 rounded-lg text-[13px] text-text-secondary font-medium transition-colors"
                  onClick={() => setDraft({ ...draft, args: [...draft.args, ""] })}
                >
                  {t("settings.mcp.addArg")}
                </button>
              </div>
            </>
          ) : (
            /* URL */
            <div>
              <div className="text-[13px] font-medium text-text-base mb-2">{t("settings.mcp.serverUrl")}</div>
              <input
                type="text"
                placeholder="https://mcp.example.com/mcp"
                value={draft.url ?? ""}
                onChange={(e) => setDraft({ ...draft, url: e.target.value })}
                className="w-full border border-border-theme rounded-lg py-2 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white"
              />
              <div className="text-[11px] text-text-secondary mt-1">
                {t("settings.mcp.urlDesc")}
              </div>
            </div>
          )}

          {/* 环境变量 */}
          <div>
            <div className="text-[13px] font-medium text-text-base mb-2">{t("settings.mcp.envVars")}</div>
            {envPairs.map((pair, i) => (
              <div key={i} className="flex items-center space-x-2 mb-2">
                <input
                  type="text"
                  placeholder={t("settings.mcp.key")}
                  value={pair.key}
                  onChange={(e) => {
                    const next = [...envPairs];
                    next[i] = { ...next[i], key: e.target.value };
                    setEnvPairs(next);
                  }}
                  className="flex-1 border border-border-theme rounded-lg py-2 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white"
                />
                <input
                  type="text"
                  placeholder={t("settings.mcp.value")}
                  value={pair.value}
                  onChange={(e) => {
                    const next = [...envPairs];
                    next[i] = { ...next[i], value: e.target.value };
                    setEnvPairs(next);
                  }}
                  className="flex-1 border border-border-theme rounded-lg py-2 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white"
                />
                <button
                  className="text-gray-400 hover:text-red-500 transition-colors px-2"
                  onClick={() => setEnvPairs(envPairs.filter((_, j) => j !== i))}
                >
                  <FontAwesomeIcon icon={["fas", "minus"]} className="text-[12px]" />
                </button>
              </div>
            ))}
            <button
              className="w-full py-1.5 bg-gray-100 hover:bg-gray-200 rounded-lg text-[13px] text-text-secondary font-medium transition-colors"
              onClick={() => setEnvPairs([...envPairs, { key: "", value: "" }])}
            >
              {t("settings.mcp.addEnvVar")}
            </button>
          </div>

          {error && <div className="text-[12px] text-red-500">{error}</div>}

          {testResult && (
            <div
              className={`rounded-lg border p-3 text-[12px] ${
                testResult.status === "connected"
                  ? "border-green-200 bg-green-50"
                  : "border-red-200 bg-red-50"
              }`}
            >
              <div className="flex items-center mb-1">
                <StatusBadge status={testResult.status} />
                {testResult.status === "connected" && (
                  <span className="ml-2 text-text-secondary">
                    {t("settings.mcp.toolsFound", { count: testResult.tools.length })}
                  </span>
                )}
              </div>
              {testResult.error && (
                <div className="font-mono text-[11px] text-red-600 break-all">{testResult.error}</div>
              )}
              {testResult.tools.length > 0 && (
                <ul className="mt-1 space-y-0.5">
                  {testResult.tools.map((tool) => (
                    <li key={tool.name} className="text-text-secondary">
                      <span className="font-mono text-text-base">{tool.name}</span>
                      {tool.description && <span className="ml-1">— {tool.description}</span>}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}

          <div className="flex justify-end items-center space-x-3 pt-4 pb-20">
            <button
              disabled={!canSave || testing}
              onClick={onTest}
              className={`px-5 py-1.5 rounded-full text-[13px] font-medium border transition-colors ${
                canSave && !testing
                  ? "border-border-theme text-text-base hover:bg-gray-100"
                  : "border-gray-200 text-gray-400 cursor-not-allowed"
              }`}
            >
              {testing && (
                <FontAwesomeIcon icon={["fas", "circle-notch"]} spin className="mr-1.5 text-[12px]" />
              )}
              {t("settings.mcp.testConnection")}
            </button>
            <button
              disabled={!canSave}
              onClick={onSave}
              className={`px-5 py-1.5 rounded-full text-[13px] font-medium transition-colors ${
                canSave
                  ? "bg-text-base text-white hover:bg-black"
                  : "bg-gray-300 text-white cursor-not-allowed"
              }`}
            >
              {t("settings.mcp.save")}
            </button>
          </div>
        </div>
      </div>
    );
  }

  // --- List View ---
  return (
    <>
      <div className="mb-10 max-w-[700px]">
        <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.mcp.title")}</h1>
        <div className="text-[13px] text-text-secondary">
          {t("settings.mcp.desc")} <a href="#" className="text-blue-500 hover:underline">{t("settings.mcp.learnMore")}</a>
        </div>
      </div>

      <div className="max-w-[700px]">
        <div className="flex items-center justify-between mb-4">
          <div className="text-[14px] font-medium text-text-base">{t("settings.mcp.servers")}</div>
          <div className="flex items-center space-x-2">
            {servers.length > 0 && (
              <button
                className="flex items-center px-3 py-1 bg-gray-100 hover:bg-gray-200 rounded-full text-[12px] font-medium text-text-base transition-colors disabled:opacity-50"
                onClick={refreshStatuses}
                disabled={checking}
                title={t("settings.mcp.refreshStatus")}
              >
                <FontAwesomeIcon
                  icon={["fas", "circle-notch"]}
                  spin={checking}
                  className="mr-1.5 text-[10px]"
                />{" "}
                {t("settings.mcp.refreshStatus")}
              </button>
            )}
            <button
              className="flex items-center px-3 py-1 bg-gray-100 hover:bg-gray-200 rounded-full text-[12px] font-medium text-text-base transition-colors"
              onClick={() => openAdd()}
            >
              <FontAwesomeIcon icon={["fas", "plus"]} className="mr-1.5 text-[10px]" /> {t("settings.mcp.addServer")}
            </button>
          </div>
        </div>

        {servers.length === 0 ? (
          <div className="border border-dashed border-border-theme rounded-xl p-8 text-center text-[13px] text-text-secondary">
            {t("settings.mcp.noServers")}
          </div>
        ) : (
          <div className="border border-border-theme rounded-xl bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] divide-y divide-border-theme">
            {servers.map((s) => {
              const st = statuses[s.name];
              const isOpen = expanded.has(s.name);
              const toolCount = st?.tools.length ?? 0;
              const canExpand = st?.status === "connected" && toolCount > 0;
              return (
                <div key={s.name}>
                  <div className="flex items-center justify-between p-4">
                    <div className="min-w-0">
                      <div className="flex items-center space-x-2">
                        <span className="text-[13px] font-mono text-text-base font-medium truncate">
                          {s.name}
                        </span>
                        {st && <StatusBadge status={st.status} />}
                        <span
                          className={`inline-flex items-center rounded-full px-2 py-0.5 text-[11px] font-medium ${
                            s.source === "plugin"
                              ? "bg-blue-50 text-blue-600"
                              : "bg-gray-100 text-gray-500"
                          }`}
                          title={s.source_path ?? undefined}
                        >
                          {sourceLabel(s)}
                        </span>
                      </div>
                      <div className="text-[11px] text-text-secondary truncate mt-0.5">
                        {s.transport} · {s.transport === "stdio" ? s.command : s.url}
                        {s.source === "plugin" && s.declared_name && s.declared_name !== s.name && (
                          <span className="ml-2">declared: {s.declared_name}</span>
                        )}
                        {canExpand && (
                          <button
                            className="ml-2 text-blue-500 hover:underline"
                            onClick={() => toggleExpanded(s.name)}
                          >
                            <FontAwesomeIcon
                              icon={["fas", isOpen ? "chevron-down" : "chevron-right"]}
                              className="mr-1 text-[9px]"
                            />
                            {t("settings.mcp.toolsFound", { count: toolCount })}
                          </button>
                        )}
                      </div>
                      {st?.status === "failed" && st.error && (
                        <div className="text-[11px] text-red-500 truncate mt-0.5" title={st.error}>
                          {st.error}
                        </div>
                      )}
                      {s.conflict && (
                        <div className="text-[11px] text-amber-600 truncate mt-0.5" title={s.conflict}>
                          {s.conflict}
                        </div>
                      )}
                    </div>
                    <div className="flex items-center space-x-4 flex-shrink-0">
                      {!s.read_only && (
                        <>
                          <button
                            className="text-gray-400 hover:text-text-base transition-colors"
                            title={t("settings.mcp.edit")}
                            onClick={() => openAdd(s)}
                          >
                            <FontAwesomeIcon icon={["fas", "gear"]} className="text-[14px]" />
                          </button>
                          <button
                            className="text-gray-400 hover:text-red-500 transition-colors"
                            title={t("settings.mcp.delete")}
                            onClick={() => onRemove(s)}
                          >
                            <FontAwesomeIcon icon={["fas", "minus"]} className="text-[14px]" />
                          </button>
                          <ToggleSwitch checked={s.enabled} onChange={() => onToggle(s)} />
                        </>
                      )}
                      {s.read_only && (
                        <span className="text-[11px] text-text-secondary">在插件页管理</span>
                      )}
                    </div>
                  </div>
                  {isOpen && canExpand && (
                    <div className="px-4 pb-4 -mt-1">
                      <ul className="rounded-lg bg-gray-50 border border-border-theme divide-y divide-border-theme">
                        {st!.tools.map((tool) => (
                          <li key={tool.name} className="p-2.5">
                            <div className="text-[12px] font-mono text-text-base">{tool.name}</div>
                            {tool.description && (
                              <div className="text-[11px] text-text-secondary mt-0.5">{tool.description}</div>
                            )}
                          </li>
                        ))}
                      </ul>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </>
  );
}
