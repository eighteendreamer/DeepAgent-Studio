import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { McpServer } from "../../types";
import {
  listMcpServers,
  saveMcpServer,
  removeMcpServer,
  setMcpServerEnabled,
} from "../../api";

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
  };
}

export function MCPSettings() {
  const { t } = useTranslation();
  const [view, setView] = useState<"list" | "add">("list");
  const [servers, setServers] = useState<McpServer[]>([]);
  const [draft, setDraft] = useState<McpServer>(emptyDraft());
  const [error, setError] = useState<string | null>(null);
  const [envPairs, setEnvPairs] = useState<{ key: string; value: string }[]>([]);

  async function refresh() {
    try {
      setServers(await listMcpServers());
    } catch {
      setServers([]);
    }
  }

  useEffect(() => {
    refresh();
  }, []);

  function openAdd(existing?: McpServer) {
    const d = existing ? { ...existing } : emptyDraft();
    setDraft(d);
    setEnvPairs(Object.entries(d.env).map(([key, value]) => ({ key, value })));
    setError(null);
    setView("add");
  }

  async function onToggle(s: McpServer) {
    await setMcpServerEnabled(s.name, !s.enabled).catch(() => {});
    refresh();
  }

  async function onRemove(name: string) {
    await removeMcpServer(name).catch(() => {});
    refresh();
  }

  async function onSave() {
    setError(null);
    const env: Record<string, string> = {};
    for (const { key, value } of envPairs) {
      if (key.trim()) env[key.trim()] = value;
    }
    const payload: McpServer = {
      ...draft,
      env,
      args: draft.args.filter((a) => a.trim() !== ""),
      command: draft.transport === "stdio" ? draft.command : null,
      url: draft.transport !== "stdio" ? draft.url : null,
    };
    try {
      await saveMcpServer(payload);
      await refresh();
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

          <div className="flex justify-end pt-4 pb-20">
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
          <button
            className="flex items-center px-3 py-1 bg-gray-100 hover:bg-gray-200 rounded-full text-[12px] font-medium text-text-base transition-colors"
            onClick={() => openAdd()}
          >
            <FontAwesomeIcon icon={["fas", "plus"]} className="mr-1.5 text-[10px]" /> {t("settings.mcp.addServer")}
          </button>
        </div>

        {servers.length === 0 ? (
          <div className="border border-dashed border-border-theme rounded-xl p-8 text-center text-[13px] text-text-secondary">
            {t("settings.mcp.noServers")}
          </div>
        ) : (
          <div className="border border-border-theme rounded-xl bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] divide-y divide-border-theme">
            {servers.map((s) => (
              <div key={s.name} className="flex items-center justify-between p-4">
                <div className="min-w-0">
                  <div className="text-[13px] font-mono text-text-base font-medium truncate">
                    {s.name}
                  </div>
                  <div className="text-[11px] text-text-secondary truncate">
                    {s.transport} · {s.transport === "stdio" ? s.command : s.url}
                  </div>
                </div>
                <div className="flex items-center space-x-4 flex-shrink-0">
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
                    onClick={() => onRemove(s.name)}
                  >
                    <FontAwesomeIcon icon={["fas", "minus"]} className="text-[14px]" />
                  </button>
                  <ToggleSwitch checked={s.enabled} onChange={() => onToggle(s)} />
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}
