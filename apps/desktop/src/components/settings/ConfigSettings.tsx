import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { GripVertical } from "lucide-react";
import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  getSettings,
  listActiveHarnessModels,
  setSandboxMode,
  type SandboxMode,
  getToolSearchMode,
  setToolSearchMode,
  type ToolSearchMode,
  getToolSearchThreshold,
  setToolSearchThreshold,
  getAnySearchApiKeyInfo,
  setAnySearchApiKey,
  clearAnySearchApiKey,
  testWebSearchEngine,
  getSkillCatalogEnabled,
  setSkillCatalogEnabled,
  getSkillCatalogCharBudget,
  setSkillCatalogCharBudget,
  getSkillInstallAiReviewEnabled,
  setSkillInstallAiReviewEnabled,
  getSkillInstallAiReviewModel,
  setSkillInstallAiReviewModel,
} from "../../api";
import type { WebSearchSettings, WebSearchEngineId } from "../../types";
import packageJson from "../../../package.json";
import { message } from "../message";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Button } from "../shadcn/button";
import { useWebSearchSettings } from "./useWebSearchSettings";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../shadcn/select";

// Sentinel "default model" option for the AI review model dropdown. Maps to
// `null` on the backend (R10.4: "default = follow chat model").
const SKILL_REVIEW_MODEL_DEFAULT = "__default__";
// Hard cap on the catalog reminder character budget input (R10.2 / task 21).
const SKILL_CATALOG_BUDGET_MAX = 32000;
function ToggleSwitch({ checked, onChange }: { checked: boolean; onChange: () => void }) {
  return (
    <div 
      className={`w-9 h-5 rounded-full relative cursor-pointer transition-colors ${checked ? 'bg-blue-500' : 'bg-gray-300'}`}
      onClick={onChange}
    >
      <div className={`w-3.5 h-3.5 rounded-full bg-white absolute top-[3px] transition-transform ${checked ? 'translate-x-[20px]' : 'translate-x-[3px]'}`} />
    </div>
  );
}

interface DropdownOption {
  title: string;
  description: string;
}

function ComplexDropdown({ 
  options, 
  selectedTitle, 
  onChange 
}: { 
  options: DropdownOption[], 
  selectedTitle: string, 
  onChange: (title: string) => void 
}) {
  const [isOpen, setIsOpen] = useState(false);

  return (
    <div className="relative">
      <div 
        className="flex items-center bg-black/5 hover:bg-black/5 rounded-lg px-3 py-1.5 cursor-pointer transition-colors w-[220px] justify-between"
        onClick={() => setIsOpen(!isOpen)}
        onBlur={() => setTimeout(() => setIsOpen(false), 200)}
        tabIndex={0}
      >
        <span className="text-[12px] font-medium text-text-base">{selectedTitle}</span>
        <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary" />
      </div>

      {isOpen && (
        <div className="absolute top-full right-0 mt-1 bg-elevated-bg rounded-xl shadow-[0_6px_24px_rgba(0,0,0,0.10)] z-20 py-2 w-[300px]">
          {options.map((opt) => (
            <div 
              key={opt.title}
              className="px-4 py-2 hover:bg-black/5 cursor-pointer flex items-center justify-between"
              onMouseDown={(e) => {
                e.preventDefault(); // Prevent blur
                onChange((opt as any).displayTitle || opt.title);
                setIsOpen(false);
              }}
            >
              <div className="flex-1 pr-4">
                <div className="text-[13px] font-medium text-text-base mb-0.5">{(opt as any).displayTitle || opt.title}</div>
                <div className="text-[12px] text-text-secondary leading-snug">{opt.description}</div>
              </div>
              <div className="w-4 flex justify-end">
                {selectedTitle === ((opt as any).displayTitle || opt.title) && (
                  <FontAwesomeIcon icon={["fas", "check"]} className="text-[12px] text-text-base" />
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export function ConfigSettings() {
  const { t } = useTranslation();
  const [dependencies, setDependencies] = useState(true);
  const [approvalStrategy, setApprovalStrategy] = useState("onDemand");
  const [sandboxSetting, setSandboxSetting] = useState<SandboxMode>("workspace_write");
  const [appVersion, setAppVersion] = useState(packageJson.version);
  // Tool-search lazy loading (tool-search spec).
  const [toolSearchModeState, setToolSearchModeState] = useState<ToolSearchMode>("disabled");
  const [toolSearchThresholdState, setToolSearchThresholdState] = useState<number>(8000);
  const [thresholdInput, setThresholdInput] = useState<string>("8000");
  const searchEditor = useWebSearchSettings(() => message.error(t("settings.config.webSearch.saveFailed")));
  const webSearchSettings = searchEditor.settings;
  const [draggingSearchEngine, setDraggingSearchEngine] = useState<WebSearchSettings["engines"][number]["id"] | null>(null);
  const [anysearchApiKeyConfigured, setAnysearchApiKeyConfigured] = useState<boolean>(false);
  const [anysearchKeyInput, setAnysearchKeyInput] = useState<string>("");
  const [testingWebSearchEngine, setTestingWebSearchEngine] = useState<WebSearchEngineId | null>(null);
  const webSearchTestInFlight = useRef(false);
  const [webSearchTestResults, setWebSearchTestResults] = useState<Partial<Record<WebSearchEngineId, { ok: boolean; error: string | null; count: number | null }>>>({});

  // Skill marketplace settings (R10.1-R10.5 / task 21).
  const [availableModels, setAvailableModels] = useState<string[]>([]);
  const [skillCatalogEnabled, setSkillCatalogEnabledState] = useState<boolean>(true);
  const [skillCatalogBudget, setSkillCatalogBudgetState] = useState<number>(8000);
  const [skillCatalogBudgetInput, setSkillCatalogBudgetInput] = useState<string>("8000");
  const [skillReviewEnabled, setSkillReviewEnabledState] = useState<boolean>(true);
  const [skillReviewModel, setSkillReviewModelState] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getSettings()
      .then((settings) => {
        if (cancelled) return;
        if (settings?.sandbox_mode) {
          setSandboxSetting(settings.sandbox_mode);
        }
      })
      .catch((e) => console.error("get_settings failed:", e));
    listActiveHarnessModels().then(models => {
      if (!cancelled) setAvailableModels(models.map(model => `${model.providerId}/${model.modelName}`));
    }).catch(error => console.error("active model list failed:", error));
    // Tool-search initial state (separate Tauri commands).
    getToolSearchMode()
      .then((m) => {
        if (!cancelled) setToolSearchModeState(m);
      })
      .catch((e) => console.error("get_tool_search_mode failed:", e));
    getToolSearchThreshold()
      .then((v) => {
        if (!cancelled) {
          setToolSearchThresholdState(v);
          setThresholdInput(String(v));
        }
      })
      .catch((e) => console.error("get_tool_search_threshold failed:", e));
    getAnySearchApiKeyInfo()
      .then((v) => {
        if (!cancelled) setAnysearchApiKeyConfigured(v.has_user_key);
      })
      .catch((e) => console.error("get_anysearch_api_key failed:", e));
    // Skill marketplace settings (R10).
    getSkillCatalogEnabled()
      .then((v) => {
        if (!cancelled) setSkillCatalogEnabledState(v);
      })
      .catch((e) => console.error("get_skill_catalog_enabled failed:", e));
    getSkillCatalogCharBudget()
      .then((v) => {
        if (!cancelled) {
          setSkillCatalogBudgetState(v);
          setSkillCatalogBudgetInput(String(v));
        }
      })
      .catch((e) => console.error("get_skill_catalog_char_budget failed:", e));
    getSkillInstallAiReviewEnabled()
      .then((v) => {
        if (!cancelled) setSkillReviewEnabledState(v);
      })
      .catch((e) => console.error("get_skill_install_ai_review_enabled failed:", e));
    getSkillInstallAiReviewModel()
      .then((v) => {
        if (!cancelled) setSkillReviewModelState(v);
      })
      .catch((e) => console.error("get_skill_install_ai_review_model failed:", e));
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let cancelled = false;
    getVersion()
      .then((version) => {
        if (!cancelled) setAppVersion(version);
      })
      .catch((e) => console.error("get app version failed:", e));
    return () => {
      cancelled = true;
    };
  }, []);

  const displayVersion = `v${appVersion.replace(/^v/i, "")}`;

  const approvalOptions = [
    { title: "untrusted", description: t("settings.config.untrustedDesc"), displayTitle: t("settings.config.untrusted") },
    { title: "onFailure", description: t("settings.config.onFailureDesc"), displayTitle: t("settings.config.onFailure") },
    { title: "onDemand", description: t("settings.config.onDemandDesc"), displayTitle: t("settings.config.onDemand") },
    { title: "never", description: t("settings.config.neverDesc"), displayTitle: t("settings.config.never") }
  ];

  const sandboxOptions = [
    { title: "read_only", description: t("settings.config.readOnlyDesc"), displayTitle: t("settings.config.readOnly") },
    { title: "workspace_write", description: t("settings.config.workspaceWriteDesc"), displayTitle: t("settings.config.workspaceWrite") },
    { title: "full_access", description: t("settings.config.fullAccessDesc"), displayTitle: t("settings.config.fullAccess") }
  ];

  const toolSearchOptions = [
    {
      title: "disabled",
      description: "全部工具立即可调（默认；最稳定）",
      displayTitle: "Disabled",
    },
    {
      title: "enabled",
      description: "MCP 工具默认隐藏，模型按需用 tool_search 检索",
      displayTitle: "Enabled",
    },
    {
      title: "auto",
      description: "仅当延迟工具的 schema 总字符数达到阈值时启用",
      displayTitle: "Auto",
    },
  ];

  const reorderSearchEngines = (
    sourceId: WebSearchSettings["engines"][number]["id"],
    targetId: WebSearchSettings["engines"][number]["id"],
  ) => {
    if (sourceId === targetId) return;
    searchEditor.update((current) => {
      const engines = [...current.engines];
      const sourceIndex = engines.findIndex((engine) => engine.id === sourceId);
      const targetIndex = engines.findIndex((engine) => engine.id === targetId);
      if (sourceIndex < 0 || targetIndex < 0) return current;
      const [source] = engines.splice(sourceIndex, 1);
      engines.splice(targetIndex, 0, source);
      return { ...current, engines };
    });
  };

  const persistAnySearchApiKey = async (nextKey: string) => {
    const trimmed = nextKey.trim();
    if (!trimmed) return false;
    try {
      await setAnySearchApiKey(trimmed);
      setAnysearchApiKeyConfigured(true);
      setAnysearchKeyInput("");
      await searchEditor.refresh();
      return true;
    } catch {
      message.error("AnySearch API Key 保存失败");
      return false;
    }
  };

  const runWebSearchEngineTest = async (engineId: WebSearchEngineId) => {
    if (webSearchTestInFlight.current) return;
    webSearchTestInFlight.current = true;
    setTestingWebSearchEngine(engineId);
    try {
      await searchEditor.flush();
      if (engineId === "anysearch" && anysearchKeyInput.trim()
        && !await persistAnySearchApiKey(anysearchKeyInput)) return;
      const result = await testWebSearchEngine(engineId);
      setWebSearchTestResults((current) => ({ ...current, [engineId]: result }));
      if (result.ok) {
        message.success(`${engineId} 测试通过：${result.count ?? 0} 条结果`);
      } else {
        message.error(result.error || `${engineId} 测试失败`);
      }
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      setWebSearchTestResults((current) => ({
        ...current,
        [engineId]: { ok: false, error: text, count: null },
      }));
      message.error(text || `${engineId} 测试失败`);
    } finally {
      webSearchTestInFlight.current = false;
      setTestingWebSearchEngine(null);
    }
  };

  const clearAnySearchApiKeyLocal = async () => {
    try {
      await clearAnySearchApiKey();
      setAnysearchApiKeyConfigured(false);
      setAnysearchKeyInput("");
      await searchEditor.refresh();
    } catch (e) {
      message.error("AnySearch API Key 清除失败");
      console.error("clear_anysearch_api_key failed:", e);
    }
  };

  return (
    <>
      <div className="mb-10">
        <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.config.title")}</h1>
        <div className="text-[13px] text-text-secondary flex items-center">
          {t("settings.config.desc")} <a href="#" className="text-blue-500 hover:underline ml-1">{t("settings.config.learnMore")}</a>
        </div>
      </div>

      {/* Section: 自定义 config.toml 设置 */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-6">{t("settings.config.customSettings")}</h2>

        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center bg-black/5 hover:bg-black/5 rounded-lg px-3 py-1.5 cursor-pointer transition-colors">
            <span className="text-[12px] font-medium text-text-base mr-2">{t("settings.config.userConfig")}</span>
            <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary" />
          </div>
          <div className="flex items-center text-[12px] text-text-secondary hover:text-text-base cursor-pointer transition-colors">
            {t("settings.config.openConfig")} <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="ml-1.5 text-[10px]" />
          </div>
        </div>

        <div className="border border-border-theme rounded-xl shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white">
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.approvalStrategy")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.config.approvalDesc")}</div>
            </div>
            <ComplexDropdown options={approvalOptions} selectedTitle={approvalOptions.find(o => o.title === approvalStrategy)?.displayTitle || ""} onChange={(t) => {
              const opt = approvalOptions.find(o => o.displayTitle === t);
              if (opt) setApprovalStrategy(opt.title);
            }} />
          </div>
          <div className="flex items-center justify-between p-4">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.sandboxSettings")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.config.sandboxDesc")}</div>
            </div>
            <ComplexDropdown options={sandboxOptions} selectedTitle={sandboxOptions.find(o => o.title === sandboxSetting)?.displayTitle || ""} onChange={async (t) => {
              const opt = sandboxOptions.find(o => o.displayTitle === t);
              if (!opt) return;
              const next = opt.title as SandboxMode;
              const previous = sandboxSetting;
              setSandboxSetting(next);
              try {
                const view = await setSandboxMode(next);
                setSandboxSetting(view.sandbox_mode);
              } catch (e) {
                setSandboxSetting(previous);
                message.error("沙箱设置保存失败");
                console.error("set_sandbox_mode failed:", e);
              }
            }} />
          </div>
        </div>
      </div>

      {/* Section: 工具懒加载（tool-search） */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-1">工具懒加载</h2>
        <div className="text-[12px] text-text-secondary mb-6">
          MCP 工具默认逐请求全量下发会占用提示词空间。开启后 MCP 工具仅以名字暴露，模型通过 <code className="font-mono px-1 py-0.5 bg-gray-100 rounded">tool_search</code> 按需获取 schema。
        </div>
        <div className="border border-border-theme rounded-xl shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white">
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">tool-search 模式</div>
              <div className="text-[12px] text-text-secondary">
                Disabled：完全等价于改前；Enabled：始终延迟；Auto：超阈值才延迟。
              </div>
            </div>
            <ComplexDropdown
              options={toolSearchOptions}
              selectedTitle={
                toolSearchOptions.find((o) => o.title === toolSearchModeState)
                  ?.displayTitle ?? "Disabled"
              }
              onChange={async (display) => {
                const opt = toolSearchOptions.find((o) => o.displayTitle === display);
                if (!opt) return;
                const next = opt.title as ToolSearchMode;
                const previous = toolSearchModeState;
                setToolSearchModeState(next);
                try {
                  const persisted = await setToolSearchMode(next);
                  setToolSearchModeState(persisted);
                } catch (e) {
                  setToolSearchModeState(previous);
                  message.error("tool-search 模式保存失败");
                  console.error("set_tool_search_mode failed:", e);
                }
              }}
            />
          </div>
          <div
            className={`flex items-center justify-between p-4 ${
              toolSearchModeState === "auto" ? "" : "opacity-50 pointer-events-none"
            }`}
          >
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">
                Auto 阈值（字符）
              </div>
              <div className="text-[12px] text-text-secondary">
                延迟工具的 schema 总字符数 ≥ 此值时启用懒加载。建议 8000–24000；空着会回到默认值 8000。
              </div>
            </div>
            <div className="flex items-center gap-2">
              <input
                type="number"
                min={1}
                step={1000}
                disabled={toolSearchModeState !== "auto"}
                value={thresholdInput}
                onChange={(e) => setThresholdInput(e.target.value)}
                onBlur={async () => {
                  const trimmed = thresholdInput.trim();
                  const previous = toolSearchThresholdState;
                  const value = trimmed === "" ? null : Number.parseInt(trimmed, 10);
                  if (value !== null && (Number.isNaN(value) || value < 1)) {
                    setThresholdInput(String(previous));
                    message.error("阈值必须为正整数");
                    return;
                  }
                  try {
                    const persisted = await setToolSearchThreshold(value);
                    setToolSearchThresholdState(persisted);
                    setThresholdInput(String(persisted));
                  } catch (e) {
                    setThresholdInput(String(previous));
                    message.error("阈值保存失败");
                    console.error("set_tool_search_threshold failed:", e);
                  }
                }}
                className="w-[110px] px-2 py-1 text-[13px] font-mono border border-border-theme rounded-md bg-white focus:outline-none focus:ring-1 focus:ring-blue-400 disabled:bg-gray-50"
              />
              <span className="text-[12px] text-text-secondary">chars</span>
            </div>
          </div>
        </div>
      </div>

      {/* Section: unified direct web_search engines */}
      {webSearchSettings ? (
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-1">web_search</h2>
        <div className="text-[12px] text-text-secondary mb-6">一个工具，按配置调用四个独立搜索引擎；聊天模型不会选择引擎。</div>
        <div className="border border-border-theme rounded-xl bg-white divide-y divide-border-theme">
          {webSearchSettings.engines.map((engine, index) => {
            const status = webSearchSettings.engine_statuses?.find((item) => item.id === engine.id)?.state;
            const statusLabel = !engine.enabled ? "已关闭" : status === "ready" ? "已开启、配置完整（未联网验证）" : "已开启、待配置";
            const label = { deepseek: "DeepSeek Search", anysearch: "AnySearch", searxng: "SearXNG", duckduckgo: "DuckDuckGo" }[engine.id];
            const needsUrl = engine.id === "anysearch" || engine.id === "searxng";
            const engineReady = engine.id === "anysearch" ? anysearchApiKeyConfigured : status === "ready";
            const testResult = webSearchTestResults[engine.id];
            return (
              <div
                key={engine.id}
                data-search-engine={engine.id}
                onDragOver={(event) => event.preventDefault()}
                onDrop={(event) => {
                  event.preventDefault();
                  const sourceId = draggingSearchEngine ?? event.dataTransfer.getData("text/plain") as WebSearchSettings["engines"][number]["id"];
                  if (sourceId) reorderSearchEngines(sourceId, engine.id);
                  setDraggingSearchEngine(null);
                }}
                className={`p-4 space-y-3 ${draggingSearchEngine && draggingSearchEngine !== engine.id ? "border-t border-dashed border-blue-300" : ""}`}
              >
                <div className="flex items-center justify-between gap-3">
                  <div className="flex items-center gap-2 min-w-0">
                    <Button
                      variant="ghost"
                      size="icon"
                      draggable
                      aria-label={`拖动调整${label}优先级`}
                      onDragStart={(event) => {
                        setDraggingSearchEngine(engine.id);
                        event.dataTransfer.effectAllowed = "move";
                        event.dataTransfer.setData("text/plain", engine.id);
                      }}
                      onDragEnd={() => setDraggingSearchEngine(null)}
                      onKeyDown={(event) => {
                        if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
                        const target = webSearchSettings.engines[index + (event.key === "ArrowUp" ? -1 : 1)];
                        if (!target) return;
                        event.preventDefault();
                        reorderSearchEngines(engine.id, target.id);
                      }}
                      className="h-7 w-7 shrink-0 rounded-md text-text-secondary hover:text-text-base cursor-grab active:cursor-grabbing"
                    ><GripVertical size={16} aria-hidden="true" /></Button>
                    <div className="min-w-0"><div className="text-[14px] font-medium text-text-base">{index + 1}. {label}</div><div className="text-[12px] text-text-secondary">{statusLabel}</div></div>
                  </div>
                  <div className="flex items-center gap-3 shrink-0">
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={!engine.enabled || !engineReady || testingWebSearchEngine !== null}
                      onClick={() => void runWebSearchEngineTest(engine.id)}
                      className="h-7 rounded-md px-3 text-[12px] disabled:cursor-not-allowed"
                    >{testingWebSearchEngine === engine.id ? "测试中…" : "测试"}</Button>
                    <Button
                      variant={engine.enabled ? "secondary" : "outline"}
                      size="sm"
                      role="switch"
                      aria-checked={engine.enabled}
                      aria-label={`${label}搜索开关`}
                      onClick={() => searchEditor.update((current) => ({ ...current, engines: current.engines.map((item) => item.id === engine.id ? { ...item, enabled: !engine.enabled } : item) }))}
                    >{engine.enabled ? "开启" : "关闭"}</Button>
                  </div>
                </div>
                {testResult && (
                  <div className={`text-[12px] ${testResult.ok ? "text-green-700" : "text-red-600"}`}>
                    {testResult.ok ? `最近测试通过：${testResult.count ?? 0} 条结果` : `最近测试失败：${testResult.error || "未知错误"}`}
                  </div>
                )}
                {needsUrl && engine.enabled && (
                  <Input type="url" aria-label={`${label}实例地址`} value={engine.base_url ?? ""} placeholder={engine.id === "anysearch" ? "https://api.anysearch.com" : "https://search.example.com"} onChange={(event) => { const base_url = event.target.value || null; searchEditor.update((current) => ({ ...current, engines: current.engines.map((item) => item.id === engine.id ? { ...item, base_url } : item) })); }} className="font-mono" />
                )}
                {engine.id === "deepseek" && <div className="text-[12px] text-text-secondary">凭据：使用当前登录 API Key</div>}
                {engine.id === "anysearch" && engine.enabled && (
                  <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-2">
                    <Input type="password" aria-label="AnySearch API Key" value={anysearchKeyInput} placeholder={anysearchApiKeyConfigured ? "已配置，重新输入可覆盖" : "请输入 AnySearch API Key"} onChange={(event) => setAnysearchKeyInput(event.target.value)} onBlur={() => void persistAnySearchApiKey(anysearchKeyInput)} className="min-w-0 font-mono" />
                    <Button variant="outline" size="sm" onClick={clearAnySearchApiKeyLocal} disabled={!anysearchApiKeyConfigured}>清除</Button>
                  </div>
                )}
              </div>
            );
          })}
        </div>
        <div className="mt-4 flex items-center gap-4 text-[12px] text-text-secondary">
          <div className="flex items-center gap-2">
            <Label id="web-search-strategy-label">策略</Label>
            <Select value={webSearchSettings.strategy} onValueChange={(value) => searchEditor.update((current) => ({ ...current, strategy: value as WebSearchSettings["strategy"] }))}>
              <SelectTrigger aria-labelledby="web-search-strategy-label" className="h-8 w-auto"><SelectValue placeholder={webSearchSettings.strategy === "verify" ? "多源核验（并行）" : "按序兜底"} /></SelectTrigger>
              <SelectContent><SelectItem value="fallback">按序兜底</SelectItem><SelectItem value="verify">多源核验（并行）</SelectItem></SelectContent>
            </Select>
          </div>
          <div className="flex items-center gap-2">
            <Label id="web-search-ranking-label">系统排序</Label>
            <Select value={webSearchSettings.ranking.profile} onValueChange={(value) => searchEditor.update((current) => ({ ...current, ranking: { ...current.ranking, profile: value as WebSearchSettings["ranking"]["profile"] } }))}>
              <SelectTrigger aria-labelledby="web-search-ranking-label" className="h-8 w-auto"><SelectValue placeholder={webSearchSettings.ranking.profile === "timely" ? "时效性" : "均衡"} /></SelectTrigger>
              <SelectContent><SelectItem value="balanced">均衡</SelectItem><SelectItem value="timely">时效性</SelectItem></SelectContent>
            </Select>
          </div>
        </div>
        <p className="mt-2 text-[12px] text-text-secondary">多源核验会增加请求数、耗时及可能费用；多个已开启引擎并行，共享现有 120 秒总预算。</p>
      </div>
      ) : <div className="mb-12" role="status">{searchEditor.loadFailed ? "搜索设置加载失败，未修改已有配置。" : "正在读取搜索设置…"}{searchEditor.loadFailed && <Button variant="outline" size="sm" onClick={() => void searchEditor.refresh()}>重试读取</Button>}</div>}

      {/* Section: 技能 (skill marketplace) */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-1">技能</h2>
        <div className="text-[12px] text-text-secondary mb-6">
          控制 skill 目录 reminder 的注入与字符预算,以及第三方 skill 安装前的 AI 安全复审。
        </div>
        <div className="border border-border-theme rounded-xl shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white">
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">技能目录 reminder</div>
              <div className="text-[12px] text-text-secondary">
                关闭后系统消息不再注入 <code className="font-mono px-1 py-0.5 bg-gray-100 rounded">&lt;available-skills&gt;</code> 块,SkillTool 仍可被模型主动调用。
              </div>
            </div>
            <ToggleSwitch
              checked={skillCatalogEnabled}
              onChange={async () => {
                const next = !skillCatalogEnabled;
                const previous = skillCatalogEnabled;
                setSkillCatalogEnabledState(next);
                try {
                  const persisted = await setSkillCatalogEnabled(next);
                  setSkillCatalogEnabledState(persisted);
                } catch (e) {
                  setSkillCatalogEnabledState(previous);
                  message.error("技能目录 reminder 保存失败");
                  console.error("set_skill_catalog_enabled failed:", e);
                }
              }}
            />
          </div>
          <div
            className={`flex items-center justify-between p-4 border-b border-border-theme ${
              skillCatalogEnabled ? "" : "opacity-50 pointer-events-none"
            }`}
          >
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">字符预算</div>
              <div className="text-[12px] text-text-secondary">
                目录 reminder 总字符上限。范围 0 - {SKILL_CATALOG_BUDGET_MAX};设为 0 等价于关闭 reminder。建议 4000 - 16000。
              </div>
            </div>
            <div className="flex items-center gap-2">
              <input
                type="number"
                min={0}
                max={SKILL_CATALOG_BUDGET_MAX}
                step={500}
                disabled={!skillCatalogEnabled}
                value={skillCatalogBudgetInput}
                onChange={(e) => setSkillCatalogBudgetInput(e.target.value)}
                onBlur={async () => {
                  const trimmed = skillCatalogBudgetInput.trim();
                  const previous = skillCatalogBudget;
                  if (trimmed === "") {
                    setSkillCatalogBudgetInput(String(previous));
                    return;
                  }
                  const parsed = Number.parseInt(trimmed, 10);
                  if (Number.isNaN(parsed) || parsed < 0 || parsed > SKILL_CATALOG_BUDGET_MAX) {
                    setSkillCatalogBudgetInput(String(previous));
                    message.error(`字符预算必须在 0 - ${SKILL_CATALOG_BUDGET_MAX} 之间`);
                    return;
                  }
                  try {
                    const persisted = await setSkillCatalogCharBudget(parsed);
                    setSkillCatalogBudgetState(persisted);
                    setSkillCatalogBudgetInput(String(persisted));
                  } catch (e) {
                    setSkillCatalogBudgetInput(String(previous));
                    message.error("字符预算保存失败");
                    console.error("set_skill_catalog_char_budget failed:", e);
                  }
                }}
                className="w-[110px] px-2 py-1 text-[13px] font-mono border border-border-theme rounded-md bg-white focus:outline-none focus:ring-1 focus:ring-blue-400 disabled:bg-gray-50"
              />
              <span className="text-[12px] text-text-secondary">chars</span>
            </div>
          </div>
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">安装前 AI 安全复审</div>
              <div className="text-[12px] text-text-secondary">
                从市场安装第三方 skill 时,在静态扫描之外额外调一次 LLM 评估风险。关闭后只看扫描报告。
              </div>
            </div>
            <ToggleSwitch
              checked={skillReviewEnabled}
              onChange={async () => {
                const next = !skillReviewEnabled;
                const previous = skillReviewEnabled;
                setSkillReviewEnabledState(next);
                try {
                  const persisted = await setSkillInstallAiReviewEnabled(next);
                  setSkillReviewEnabledState(persisted);
                } catch (e) {
                  setSkillReviewEnabledState(previous);
                  message.error("AI 安全复审开关保存失败");
                  console.error("set_skill_install_ai_review_enabled failed:", e);
                }
              }}
            />
          </div>
          <div
            className={`flex items-center justify-between p-4 ${
              skillReviewEnabled ? "" : "opacity-50 pointer-events-none"
            }`}
          >
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">复审使用的模型</div>
              <div className="text-[12px] text-text-secondary">
                选择默认时,AI 复审跟随当前 chat 模型;否则固定使用所选模型。
              </div>
            </div>
            {(() => {
              const reviewOptions: DropdownOption[] = [
                {
                  title: SKILL_REVIEW_MODEL_DEFAULT,
                  description: "复审跟随当前 chat 模型",
                  ...({ displayTitle: "默认（跟随 chat 模型）" } as object),
                },
                ...availableModels.map((m) => ({
                  title: m,
                  description: `固定使用 ${m}`,
                  ...({ displayTitle: m } as object),
                })),
              ];
              const selectedTitle =
                skillReviewModel === null
                  ? "默认（跟随 chat 模型）"
                  : (reviewOptions.find(
                      (o) => (o as { displayTitle?: string }).displayTitle === skillReviewModel
                    ) as { displayTitle?: string } | undefined)?.displayTitle ?? skillReviewModel;
              return (
                <ComplexDropdown
                  options={reviewOptions}
                  selectedTitle={selectedTitle}
                  onChange={async (display) => {
                    const next: string | null =
                      display === "默认（跟随 chat 模型）" ? null : display;
                    const previous = skillReviewModel;
                    setSkillReviewModelState(next);
                    try {
                      const persisted = await setSkillInstallAiReviewModel(next);
                      setSkillReviewModelState(persisted);
                    } catch (e) {
                      setSkillReviewModelState(previous);
                      message.error("复审模型保存失败");
                      console.error("set_skill_install_ai_review_model failed:", e);
                    }
                  }}
                />
              );
            })()}
          </div>
        </div>
      </div>

      {/* Section: 工作空间依赖项 */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-4">{t("settings.config.dependencies")}</h2>
        <div className="border border-border-theme rounded-xl overflow-hidden shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white">
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div className="text-[14px] font-medium text-text-base">{t("settings.config.currentVersion")}</div>
            <div className="text-[13px] text-text-secondary font-mono">{displayVersion}</div>
          </div>
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.codexDependencies")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.config.codexDependenciesDesc")}</div>
            </div>
            <ToggleSwitch checked={dependencies} onChange={() => setDependencies(!dependencies)} />
          </div>
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.diagnose")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.config.diagnoseDesc")}</div>
            </div>
            <button className="flex items-center px-4 py-1.5 bg-black/5 hover:bg-black/5 rounded-md text-[12px] font-medium text-text-base transition-colors shadow-sm">
              <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className="mr-2 text-[11px] text-text-secondary" /> {t("settings.config.diagnoseBtn")}
            </button>
          </div>
          <div className="flex items-center justify-between p-4">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.resetWorkspace")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.config.resetDesc")}</div>
            </div>
            <button className="flex items-center px-4 py-1.5 bg-red-50/50 hover:bg-red-50 border border-red-200 rounded-md text-[12px] font-medium text-red-500 transition-colors shadow-sm">
              <FontAwesomeIcon icon={["fas", "download"]} className="mr-2 text-[11px]" /> {t("settings.config.reinstall")}
            </button>
          </div>
        </div>
      </div>
    </>
  );
}
