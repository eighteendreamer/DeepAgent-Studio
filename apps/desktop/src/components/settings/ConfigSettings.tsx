import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";
import {
  getSettings,
  setSandboxMode,
  type SandboxMode,
  getToolSearchMode,
  setToolSearchMode,
  type ToolSearchMode,
  getToolSearchThreshold,
  setToolSearchThreshold,
  getWebSearchSettings,
  getAnySearchApiKeyInfo,
  getVisionSettings,
  setAnySearchApiKey,
  clearAnySearchApiKey,
  testAnySearchApiKey,
  setWebSearchSettings,
  setVisionSettings,
  getSkillCatalogEnabled,
  setSkillCatalogEnabled,
  getSkillCatalogCharBudget,
  setSkillCatalogCharBudget,
  getSkillInstallAiReviewEnabled,
  setSkillInstallAiReviewEnabled,
  getSkillInstallAiReviewModel,
  setSkillInstallAiReviewModel,
  runtimeCancel,
  runtimeInstall,
  runtimeList,
  runtimeMigrateResources,
  runtimeRoots,
  runtimeProgressSubscribe,
  runtimeUninstall,
} from "../../api";
import type {
  RuntimeProgress,
  RuntimeRoots,
  RuntimeStatus,
  VisionSettings,
  WebSearchProvider,
  WebSearchSettings,
} from "../../types";
import packageJson from "../../../package.json";
import { message } from "../message";

// Sentinel "default model" option for the AI review model dropdown. Maps to
// `null` on the backend (R10.4: "default = follow chat model").
const SKILL_REVIEW_MODEL_DEFAULT = "__default__";
// Hard cap on the catalog reminder character budget input (R10.2 / task 21).
const SKILL_CATALOG_BUDGET_MAX = 32000;
const ANYSEARCH_DEFAULT_BASE_URL = "https://api.anysearch.com";

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
        className="flex items-center bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg px-3 py-1.5 cursor-pointer transition-colors w-[220px] justify-between"
        onClick={() => setIsOpen(!isOpen)}
        onBlur={() => setTimeout(() => setIsOpen(false), 200)}
        tabIndex={0}
      >
        <span className="text-[12px] font-medium text-text-base">{selectedTitle}</span>
        <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary" />
      </div>

      {isOpen && (
        <div className="absolute top-full right-0 mt-1 bg-white border border-border-theme rounded-xl shadow-lg z-20 py-2 w-[300px]">
          {options.map((opt) => (
            <div 
              key={opt.title}
              className="px-4 py-2 hover:bg-gray-50 cursor-pointer flex items-center justify-between"
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

const RUNTIME_CAPABILITY_LABEL: Record<string, string> = {
  "speech-model": "语音模型",
  "speech-engine": "转写引擎",
  "doc-convert": "文档转换",
  "pdf-render": "PDF 渲染",
  "office-suite": "Office 套件",
  "vision-image-to-text": "系统视觉",
};

const RUNTIME_DESCRIPTIONS: Record<string, string> = {
  "whisper-base": "默认本地语音转文字模型，适合会议录音、语音备忘和日常转写。",
  "whisper-small": "更高质量的 Whisper 模型，体积更大，适合对准确率要求更高的录音。",
  "whisper-cli": "whisper.cpp 本地转写引擎，语音模型需要通过它在本机执行。",
  pandoc: "增强 Markdown、docx 等文档转换能力。",
  pdfium: "增强 PDF 页面预览和栅格化渲染能力。",
  libreoffice: "用于旧版 Office 格式、高保真转换和导出能力。",
};

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "未知大小";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

function runtimeProgressPercent(progress?: RuntimeProgress): number | null {
  if (!progress?.total || progress.total <= 0) return null;
  return Math.min(100, Math.round((progress.downloaded / progress.total) * 100));
}

function RuntimeResourceSettings() {
  const [runtimes, setRuntimes] = useState<RuntimeStatus[]>([]);
  const [roots, setRoots] = useState<RuntimeRoots | null>(null);
  const [loading, setLoading] = useState(true);
  const [busyIds, setBusyIds] = useState<Set<string>>(new Set());
  const [progress, setProgress] = useState<Record<string, RuntimeProgress>>({});

  const refresh = () => {
    setLoading(true);
    Promise.all([runtimeList(), runtimeRoots()])
      .then(([items, nextRoots]) => {
        setRuntimes(items);
        setRoots(nextRoots);
      })
      .catch((e) => {
        console.error("runtime resources failed:", e);
        message.error("资源列表加载失败");
      })
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    refresh();
  }, []);

  const setBusy = (id: string, value: boolean) => {
    setBusyIds((prev) => {
      const next = new Set(prev);
      if (value) next.add(id);
      else next.delete(id);
      return next;
    });
  };

  const installRuntime = async (runtime: RuntimeStatus) => {
    setBusy(runtime.id, true);
    const unlisten = await runtimeProgressSubscribe(runtime.id, (p) => {
      setProgress((prev) => ({ ...prev, [runtime.id]: p }));
    });
    try {
      await runtimeInstall(runtime.id);
      message.success(`${runtime.name} 已安装`);
      refresh();
    } catch (e) {
      console.error("runtime_install failed:", e);
      message.error(`${runtime.name} 安装失败`);
    } finally {
      unlisten();
      setBusy(runtime.id, false);
      setProgress((prev) => {
        const next = { ...prev };
        delete next[runtime.id];
        return next;
      });
    }
  };

  const uninstallRuntime = async (runtime: RuntimeStatus) => {
    if (!window.confirm(`卸载 ${runtime.name}？\n\n卸载后相关功能下次使用前需要重新下载。`)) {
      return;
    }
    setBusy(runtime.id, true);
    try {
      await runtimeUninstall(runtime.id);
      message.success(`${runtime.name} 已卸载`);
      refresh();
    } catch (e) {
      console.error("runtime_uninstall failed:", e);
      message.error(`${runtime.name} 卸载失败`);
    } finally {
      setBusy(runtime.id, false);
    }
  };

  const cancelRuntime = async (runtime: RuntimeStatus) => {
    try {
      await runtimeCancel(runtime.id);
      message.info(`已请求取消 ${runtime.name}`);
    } catch (e) {
      console.error("runtime_cancel failed:", e);
      message.error("取消失败");
    }
  };

  const migrateResources = async () => {
    setLoading(true);
    try {
      const items = await runtimeMigrateResources();
      setRuntimes(items);
      setRoots(await runtimeRoots());
      message.success("资源已迁移到当前资源目录");
    } catch (e) {
      console.error("runtime_migrate_resources failed:", e);
      message.error("资源迁移失败，旧目录仍会继续作为兼容读取目录");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="mb-12 max-w-[700px]">
      <div className="flex items-end justify-between mb-4">
        <div>
          <h2 className="text-[15px] font-medium text-text-base mb-1">按需下载资源</h2>
          <div className="text-[12px] text-text-secondary">
            管理本地语音模型、转写引擎、文档转换、PDF 渲染和视觉模型等可选运行时。新资源安装在当前资源目录中，旧目录仅作为兼容读取来源。
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {roots && roots.fallback_roots.length > 0 && (
            <button
              type="button"
              onClick={migrateResources}
              disabled={loading}
              className="flex items-center px-3 py-1.5 rounded-lg border border-border-theme text-[12px] text-text-base hover:bg-gray-50 disabled:opacity-50"
            >
              <FontAwesomeIcon icon={["fas", "download"]} className="mr-2 text-[11px]" />
              迁移资源
            </button>
          )}
          <button
            type="button"
            onClick={refresh}
            disabled={loading}
            className="flex items-center px-3 py-1.5 rounded-lg border border-border-theme text-[12px] text-text-base hover:bg-gray-50 disabled:opacity-50"
          >
            <FontAwesomeIcon icon={["fas", "rotate-right"]} className={`mr-2 text-[11px] ${loading ? "animate-spin" : ""}`} />
            刷新
          </button>
        </div>
      </div>

      <div className="border border-border-theme rounded-xl shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white overflow-hidden">
        {loading && runtimes.length === 0 ? (
          <div className="p-4 text-[13px] text-text-secondary">正在加载资源列表...</div>
        ) : runtimes.length === 0 ? (
          <div className="p-4 text-[13px] text-text-secondary">暂无可管理资源。</div>
        ) : (
          runtimes.map((runtime, index) => {
            const isBusy = busyIds.has(runtime.id);
            const currentProgress = progress[runtime.id];
            const pct = runtimeProgressPercent(currentProgress);
            const installDisabled =
              isBusy ||
              runtime.installed ||
              !runtime.available_for_platform ||
              !runtime.checksum_pinned;
            const unavailableReason = !runtime.available_for_platform
              ? "当前系统不可用"
              : !runtime.checksum_pinned
              ? "等待校验值固定"
              : null;

            return (
              <div
                key={runtime.id}
                className={`p-4 ${index === runtimes.length - 1 ? "" : "border-b border-border-theme"}`}
              >
                <div className="flex items-start justify-between gap-4">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2 mb-1">
                      <span className="text-[14px] font-medium text-text-base">{runtime.name}</span>
                      <span className="rounded-md bg-gray-100 px-2 py-0.5 text-[11px] text-text-secondary">
                        {RUNTIME_CAPABILITY_LABEL[runtime.capability] ?? runtime.capability}
                      </span>
                      <span
                        className={`rounded-md px-2 py-0.5 text-[11px] ${
                          runtime.installed
                            ? "bg-green-50 text-green-600"
                            : "bg-gray-100 text-text-secondary"
                        }`}
                      >
                        {runtime.installed ? "已安装" : "未安装"}
                      </span>
                    </div>
                    <div className="text-[12px] text-text-secondary leading-relaxed">
                      {RUNTIME_DESCRIPTIONS[runtime.id] ?? "按需下载的本地运行时资源。"}
                    </div>
                    <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-text-secondary">
                      <span>ID: <code className="font-mono">{runtime.id}</code></span>
                      <span>版本: {runtime.version}</span>
                      <span>大小: {formatBytes(runtime.size_bytes)}</span>
                      {runtime.install_path && (
                        <span className="max-w-full truncate">
                          路径: {runtime.install_path}
                          {runtime.install_source === "fallback"
                            ? "（旧目录）"
                            : runtime.install_source === "system"
                              ? "（系统安装）"
                              : ""}
                        </span>
                      )}
                      {unavailableReason && <span className="text-amber-600">{unavailableReason}</span>}
                    </div>
                    {currentProgress && (
                      <div className="mt-3">
                        <div className="h-1.5 overflow-hidden rounded-full bg-gray-100">
                          <div
                            className="h-full bg-primary transition-all"
                            style={{ width: `${pct ?? 8}%` }}
                          />
                        </div>
                        <div className="mt-1 text-[11px] text-text-secondary">
                          {currentProgress.phase === "downloading" ? "下载中" : currentProgress.phase}
                          {pct !== null ? ` ${pct}%` : ""}
                          {" · "}
                          {formatBytes(currentProgress.downloaded)}
                          {currentProgress.total ? ` / ${formatBytes(currentProgress.total)}` : ""}
                        </div>
                      </div>
                    )}
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    {isBusy && !runtime.installed ? (
                      <button
                        type="button"
                        onClick={() => cancelRuntime(runtime)}
                        className="rounded-lg border border-border-theme px-3 py-1.5 text-[12px] text-text-base hover:bg-gray-50"
                      >
                        取消
                      </button>
                    ) : runtime.installed ? (
                      <button
                        type="button"
                        onClick={() => uninstallRuntime(runtime)}
                        disabled={isBusy}
                        className="rounded-lg border border-red-200 bg-red-50/50 px-3 py-1.5 text-[12px] font-medium text-red-500 hover:bg-red-50 disabled:opacity-50"
                      >
                        卸载
                      </button>
                    ) : (
                      <button
                        type="button"
                        onClick={() => installRuntime(runtime)}
                        disabled={installDisabled}
                        className="rounded-lg bg-primary px-3 py-1.5 text-[12px] font-medium text-white hover:opacity-90 disabled:bg-gray-200 disabled:text-text-secondary"
                        title={unavailableReason ?? undefined}
                      >
                        下载
                      </button>
                    )}
                  </div>
                </div>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
function VisionResourceSettings() {
  const [settings, setSettingsState] = useState<VisionSettings>({
    mode: "system",
    provider: "modelscope",
    base_url: "https://api-inference.modelscope.cn/v1",
    api_key: null,
    api_key_configured: false,
    system_model: "moonshotai/Kimi-K2.5:DashScope",
    timeout_ms: 60000,
    auto_analyze_pasted_images: true,
    send_original_image_to_model: false,
  });
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [loading, setLoading] = useState(true);
  const [testing, setTesting] = useState(false);

  useEffect(() => {
    let cancelled = false;
    getVisionSettings()
      .then((value) => {
        if (!cancelled) setSettingsState(value);
      })
      .catch((e) => {
        console.error("get_vision_settings failed:", e);
        message.error("视觉设置加载失败");
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const persist = async (next: VisionSettings) => {
    const previous = settings;
    setSettingsState(next);
    try {
      const saved = await setVisionSettings(next);
      setSettingsState(saved);
      if (next.api_key !== undefined) setApiKeyInput("");
    } catch (e) {
      setSettingsState(previous);
      message.error("视觉设置保存失败");
      console.error("set_vision_settings failed:", e);
    }
  };

  const providerOptions = [
    {
      title: "modelscope",
      displayTitle: "魔搭社区",
      description: "使用 ModelScope API-Inference 的 OpenAI-compatible 接口。",
    },
    {
      title: "openai_compatible",
      displayTitle: "OpenAI 兼容",
      description: "自定义兼容 /v1/chat/completions 的视觉模型服务。",
    },
  ];

  const disabled = loading || settings.mode === "off";

  return (
    <div className="mb-12 max-w-[700px]">
      <div className="mb-4">
        <h2 className="text-[15px] font-medium text-text-base mb-1">系统视觉</h2>
        <div className="text-[12px] text-text-secondary">
          用第三方视觉模型识别截图、图片、界面和图表，识别结果会作为文本上下文交给当前主模型。
        </div>
      </div>
      <div className={`border border-border-theme rounded-xl bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] ${loading ? "opacity-60" : ""}`}>
        <div className="flex items-center justify-between p-4 border-b border-border-theme">
          <div>
            <div className="text-[14px] font-medium text-text-base mb-1">启用系统视觉</div>
            <div className="text-[12px] text-text-secondary">开启后，图片附件会先发送给视觉模型识别。</div>
          </div>
          <ToggleSwitch
            checked={settings.mode === "system"}
            onChange={() =>
              void persist({
                ...settings,
                mode: settings.mode === "system" ? "off" : "system",
              })
            }
          />
        </div>
        <div className={`flex items-center justify-between p-4 border-b border-border-theme ${disabled ? "opacity-50 pointer-events-none" : ""}`}>
          <div>
            <div className="text-[14px] font-medium text-text-base mb-1">视觉供应商</div>
            <div className="text-[12px] text-text-secondary">默认使用魔搭社区，也可以填写 OpenAI 兼容服务。</div>
          </div>
          <ComplexDropdown
            options={providerOptions}
            selectedTitle={providerOptions.find((item) => item.title === settings.provider)?.displayTitle ?? "魔搭社区"}
            onChange={(display) => {
              const selected = providerOptions.find((item) => item.displayTitle === display);
              if (!selected) return;
              void persist({
                ...settings,
                provider: selected.title,
                base_url:
                  selected.title === "modelscope"
                    ? "https://api-inference.modelscope.cn/v1"
                    : settings.base_url,
              });
            }}
          />
        </div>
        <div className={`flex items-center justify-between p-4 border-b border-border-theme ${disabled ? "opacity-50 pointer-events-none" : ""}`}>
          <div>
            <div className="text-[14px] font-medium text-text-base mb-1">API 地址</div>
            <div className="text-[12px] text-text-secondary">例如 https://api-inference.modelscope.cn/v1</div>
          </div>
          <input
            type="url"
            value={settings.base_url}
            onChange={(e) => setSettingsState({ ...settings, base_url: e.target.value })}
            onBlur={() => void persist({ ...settings, base_url: settings.base_url.trim() })}
            className="w-[320px] px-2 py-1 text-[13px] font-mono border border-border-theme rounded-md bg-white focus:outline-none focus:ring-1 focus:ring-blue-400"
          />
        </div>
        <div className={`flex items-center justify-between p-4 border-b border-border-theme ${disabled ? "opacity-50 pointer-events-none" : ""}`}>
          <div>
            <div className="text-[14px] font-medium text-text-base mb-1">API Key</div>
            <div className="text-[12px] text-text-secondary">
              {settings.api_key_configured ? "已保存密钥。留空不会覆盖，输入新密钥后失焦保存。" : "请输入视觉供应商的 API Key。"}
            </div>
          </div>
          <input
            type="password"
            value={apiKeyInput}
            placeholder={settings.api_key_configured ? "已配置" : "ModelScope Token"}
            onChange={(e) => setApiKeyInput(e.target.value)}
            onBlur={() => {
              if (apiKeyInput.trim()) {
                void persist({ ...settings, api_key: apiKeyInput.trim() });
              }
            }}
            className="w-[320px] px-2 py-1 text-[13px] font-mono border border-border-theme rounded-md bg-white focus:outline-none focus:ring-1 focus:ring-blue-400"
          />
        </div>
        <div className={`flex items-center justify-between p-4 border-b border-border-theme ${disabled ? "opacity-50 pointer-events-none" : ""}`}>
          <div>
            <div className="text-[14px] font-medium text-text-base mb-1">视觉模型名称</div>
            <div className="text-[12px] text-text-secondary">填写魔搭 Model-Id，例如 moonshotai/Kimi-K2.5:DashScope</div>
          </div>
          <input
            type="text"
            value={settings.system_model}
            onChange={(e) => setSettingsState({ ...settings, system_model: e.target.value })}
            onBlur={() => void persist({ ...settings, system_model: settings.system_model.trim() })}
            className="w-[320px] px-2 py-1 text-[13px] font-mono border border-border-theme rounded-md bg-white focus:outline-none focus:ring-1 focus:ring-blue-400"
          />
        </div>
        <div className={`flex items-center justify-between p-4 border-b border-border-theme ${disabled ? "opacity-50 pointer-events-none" : ""}`}>
          <div>
            <div className="text-[14px] font-medium text-text-base mb-1">自动识别图片</div>
            <div className="text-[12px] text-text-secondary">发送消息时再调用系统视觉，避免未发送的图片消耗额度。</div>
          </div>
          <ToggleSwitch
            checked={settings.auto_analyze_pasted_images}
            onChange={() =>
              void persist({
                ...settings,
                auto_analyze_pasted_images: !settings.auto_analyze_pasted_images,
              })
            }
          />
        </div>
        <div className={`flex items-center justify-between p-4 ${disabled ? "opacity-50 pointer-events-none" : ""}`}>
          <div>
            <div className="text-[14px] font-medium text-text-base mb-1">测试连接</div>
            <div className="text-[12px] text-text-secondary">使用魔搭示例图片测试当前 API Key 和模型是否可用。</div>
          </div>
          <button
            type="button"
            disabled={testing}
            onClick={async () => {
              setTesting(true);
              try {
                const { visionTestConnection } = await import("../../api");
                const result = await visionTestConnection();
                message.success(result.text ? "视觉模型测试成功" : "视觉模型已响应");
              } catch (e) {
                message.error(`视觉模型测试失败：${String(e)}`);
              } finally {
                setTesting(false);
              }
            }}
            className="px-3 py-1.5 text-[13px] rounded-md border border-border-theme bg-white hover:bg-gray-50 disabled:opacity-50"
          >
            {testing ? "测试中..." : "测试连接"}
          </button>
        </div>
      </div>
    </div>
  );
}

import { useTranslation } from "react-i18next";

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
  const [webSearchSettings, setWebSearchSettingsState] = useState<WebSearchSettings>({
    enabled: true,
    provider: "deepseek_first",
    searxng_url: null,
    anysearch_enabled: false,
    anysearch_base_url: null,
    anysearch_api_key_configured: false,
  });
  const [searxngInput, setSearxngInput] = useState<string>("");
  const [anysearchBaseUrlInput, setAnysearchBaseUrlInput] = useState<string>(ANYSEARCH_DEFAULT_BASE_URL);
  const [anysearchApiKeyConfigured, setAnysearchApiKeyConfigured] = useState<boolean>(false);
  const [anysearchKeyInput, setAnysearchKeyInput] = useState<string>("");

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
        if (settings?.available_models) {
          setAvailableModels(settings.available_models);
        }
      })
      .catch((e) => console.error("get_settings failed:", e));
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
    getWebSearchSettings()
      .then((v) => {
        if (!cancelled) {
          setWebSearchSettingsState(v);
          setSearxngInput(v.searxng_url ?? "");
          setAnysearchBaseUrlInput(v.anysearch_base_url ?? ANYSEARCH_DEFAULT_BASE_URL);
          setAnysearchApiKeyConfigured(v.anysearch_api_key_configured);
        }
      })
      .catch((e) => console.error("get_web_search_settings failed:", e));
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

  const webSearchOptions = [
    {
      title: "deepseek_first",
      description: t("settings.config.webSearch.providers.deepseekFirstDesc"),
      displayTitle: t("settings.config.webSearch.providers.deepseekFirst"),
    },
    {
      title: "searxng",
      description: t("settings.config.webSearch.providers.searxngDesc"),
      displayTitle: t("settings.config.webSearch.providers.searxng"),
    },
    {
      title: "duckduckgo",
      description: t("settings.config.webSearch.providers.duckduckgoDesc"),
      displayTitle: t("settings.config.webSearch.providers.duckduckgo"),
    },
  ];

  const persistWebSearchSettings = async (next: WebSearchSettings) => {
    const previous = webSearchSettings;
    setWebSearchSettingsState(next);
    try {
      const persisted = await setWebSearchSettings(next);
      setWebSearchSettingsState(persisted);
      setSearxngInput(persisted.searxng_url ?? "");
      setAnysearchBaseUrlInput(persisted.anysearch_base_url ?? ANYSEARCH_DEFAULT_BASE_URL);
      setAnysearchApiKeyConfigured(persisted.anysearch_api_key_configured);
    } catch (e) {
      setWebSearchSettingsState(previous);
      setSearxngInput(previous.searxng_url ?? "");
      setAnysearchBaseUrlInput(previous.anysearch_base_url ?? ANYSEARCH_DEFAULT_BASE_URL);
      setAnysearchApiKeyConfigured(previous.anysearch_api_key_configured);
      message.error(t("settings.config.webSearch.saveFailed"));
      console.error("set_web_search_settings failed:", e);
    }
  };

  const persistAnySearchApiKey = async (nextKey: string) => {
    const trimmed = nextKey.trim();
    if (!trimmed) return;
    try {
      await setAnySearchApiKey(trimmed);
      setAnysearchApiKeyConfigured(true);
      setWebSearchSettingsState((prev) => ({ ...prev, anysearch_api_key_configured: true }));
      setAnysearchKeyInput("");
    } catch (e) {
      message.error("AnySearch API Key 保存失败");
      console.error("set_anysearch_api_key failed:", e);
    }
  };

  const clearAnySearchApiKeyLocal = async () => {
    try {
      await clearAnySearchApiKey();
      setAnysearchApiKeyConfigured(false);
      setWebSearchSettingsState((prev) => ({ ...prev, anysearch_api_key_configured: false }));
      setAnysearchKeyInput("");
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
          <div className="flex items-center bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg px-3 py-1.5 cursor-pointer transition-colors">
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

      {/* Section: web_search provider */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-1">{t("settings.config.webSearch.title")}</h2>
        <div className="text-[12px] text-text-secondary mb-6">
          {t("settings.config.webSearch.descBefore")} <code className="font-mono px-1 py-0.5 bg-gray-100 rounded">web_search</code> {t("settings.config.webSearch.descAfter")}
        </div>
        <div className="border border-border-theme rounded-xl shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white">
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.webSearch.enableTitle")}</div>
              <div className="text-[12px] text-text-secondary">
                {t("settings.config.webSearch.enableDescBefore")} <code className="font-mono px-1 py-0.5 bg-gray-100 rounded">web_fetch</code> {t("settings.config.webSearch.enableDescAfter")}
              </div>
            </div>
            <ToggleSwitch
              checked={webSearchSettings.enabled}
              onChange={() =>
                persistWebSearchSettings({
                  ...webSearchSettings,
                  enabled: !webSearchSettings.enabled,
                  searxng_url: searxngInput.trim() || null,
                })
              }
            />
          </div>
          <div
            className={`flex items-center justify-between p-4 border-b border-border-theme ${
              webSearchSettings.enabled ? "" : "opacity-50 pointer-events-none"
            }`}
          >
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">优先使用 AnySearch</div>
              <div className="text-[12px] text-text-secondary">
                启用后，web_search 会先尝试 AnySearch，再回退到当前 provider 链。
              </div>
            </div>
            <ToggleSwitch
              checked={webSearchSettings.anysearch_enabled}
              onChange={() =>
                persistWebSearchSettings({
                  ...webSearchSettings,
                  anysearch_enabled: !webSearchSettings.anysearch_enabled,
                  anysearch_base_url: anysearchBaseUrlInput.trim() || null,
                  searxng_url: searxngInput.trim() || null,
                })
              }
            />
          </div>
          <div
            className={`flex items-center justify-between p-4 border-b border-border-theme ${
              webSearchSettings.enabled && webSearchSettings.anysearch_enabled
                ? ""
                : "opacity-50 pointer-events-none"
            }`}
          >
            <div className="min-w-0 pr-4">
              <div className="text-[14px] font-medium text-text-base mb-1">AnySearch API Key</div>
              <div className="text-[12px] text-text-secondary">
                只保存到系统密钥串，不写入数据库；未配置时会继续回退到后备 provider。
              </div>
            </div>
            <div className="flex items-center gap-2">
              <input
                type="password"
                value={anysearchKeyInput}
                placeholder={anysearchApiKeyConfigured ? "已配置，重新输入可覆盖" : "请输入 AnySearch API Key"}
                disabled={!webSearchSettings.enabled || !webSearchSettings.anysearch_enabled}
                onChange={(e) => setAnysearchKeyInput(e.target.value)}
                onBlur={() => {
                  void persistAnySearchApiKey(anysearchKeyInput);
                }}
                className="w-[260px] px-2 py-1 text-[13px] font-mono border border-border-theme rounded-md bg-white focus:outline-none focus:ring-1 focus:ring-blue-400 disabled:bg-gray-50"
              />
              <button
                type="button"
                onClick={async () => {
                  if (anysearchKeyInput.trim()) {
                    await persistAnySearchApiKey(anysearchKeyInput);
                  }
                  const result = await testAnySearchApiKey();
                  if (result.ok) {
                    message.success(`AnySearch 可用${result.provider ? ` (${result.provider})` : ""}`);
                  } else {
                    message.error(result.error || "AnySearch 测试失败");
                  }
                }}
                disabled={!webSearchSettings.enabled || !webSearchSettings.anysearch_enabled}
                className="px-3 py-1.5 text-[12px] font-medium text-text-base bg-gray-100 border border-border-theme rounded-md hover:bg-gray-200 disabled:opacity-50"
              >
                测试
              </button>
              <button
                type="button"
                onClick={clearAnySearchApiKeyLocal}
                disabled={!anysearchApiKeyConfigured}
                className="px-3 py-1.5 text-[12px] font-medium text-text-base bg-white border border-border-theme rounded-md hover:bg-gray-50 disabled:opacity-50"
              >
                清除
              </button>
            </div>
          </div>
          <div
            className={`flex items-center justify-between p-4 border-b border-border-theme ${
              webSearchSettings.enabled && webSearchSettings.anysearch_enabled
                ? ""
                : "opacity-50 pointer-events-none"
            }`}
          >
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">AnySearch Base URL</div>
              <div className="text-[12px] text-text-secondary">
                默认使用 https://api.anysearch.com，可按需改成自建网关。
              </div>
            </div>
            <input
              type="url"
              value={anysearchBaseUrlInput}
              placeholder={ANYSEARCH_DEFAULT_BASE_URL}
              disabled={!webSearchSettings.enabled || !webSearchSettings.anysearch_enabled}
              onChange={(e) => setAnysearchBaseUrlInput(e.target.value)}
              onBlur={() =>
                persistWebSearchSettings({
                  ...webSearchSettings,
                  anysearch_enabled: webSearchSettings.anysearch_enabled,
                  anysearch_base_url: anysearchBaseUrlInput.trim() || null,
                  searxng_url: searxngInput.trim() || null,
                })
              }
              className="w-[260px] px-2 py-1 text-[13px] font-mono border border-border-theme rounded-md bg-white focus:outline-none focus:ring-1 focus:ring-blue-400 disabled:bg-gray-50"
            />
          </div>
          <div
            className={`flex items-center justify-between p-4 border-b border-border-theme ${
              webSearchSettings.enabled ? "" : "opacity-50 pointer-events-none"
            }`}
          >
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.webSearch.providerTitle")}</div>
              <div className="text-[12px] text-text-secondary">
                {t("settings.config.webSearch.providerDesc")}
              </div>
            </div>
            <ComplexDropdown
              options={webSearchOptions}
              selectedTitle={
                webSearchOptions.find((o) => o.title === webSearchSettings.provider)
                  ?.displayTitle ?? t("settings.config.webSearch.providers.deepseekFirst")
              }
              onChange={(display) => {
                const opt = webSearchOptions.find((o) => o.displayTitle === display);
                if (!opt) return;
                persistWebSearchSettings({
                  ...webSearchSettings,
                  provider: opt.title as WebSearchProvider,
                  searxng_url: searxngInput.trim() || null,
                });
              }}
            />
          </div>
          <div
            className={`flex items-center justify-between p-4 ${
              webSearchSettings.enabled && webSearchSettings.provider !== "duckduckgo"
                ? ""
                : "opacity-50 pointer-events-none"
            }`}
          >
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.webSearch.searxngUrlTitle")}</div>
              <div className="text-[12px] text-text-secondary">
                {t("settings.config.webSearch.searxngUrlDescBefore")} <code className="font-mono px-1 py-0.5 bg-gray-100 rounded">https://search.example.com</code>.
              </div>
            </div>
            <input
              type="url"
              value={searxngInput}
              placeholder="https://search.example.com"
              disabled={!webSearchSettings.enabled || webSearchSettings.provider === "duckduckgo"}
              onChange={(e) => setSearxngInput(e.target.value)}
              onBlur={() =>
                persistWebSearchSettings({
                  ...webSearchSettings,
                  searxng_url: searxngInput.trim() || null,
                })
              }
              className="w-[260px] px-2 py-1 text-[13px] font-mono border border-border-theme rounded-md bg-white focus:outline-none focus:ring-1 focus:ring-blue-400 disabled:bg-gray-50"
            />
          </div>
        </div>
      </div>

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

      <RuntimeResourceSettings />
      <VisionResourceSettings />

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
            <button className="flex items-center px-4 py-1.5 bg-gray-50 hover:bg-gray-100 border border-border-theme rounded-md text-[12px] font-medium text-text-base transition-colors shadow-sm">
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
