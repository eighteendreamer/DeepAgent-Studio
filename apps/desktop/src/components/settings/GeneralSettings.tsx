import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useState, useRef, useEffect } from "react";
import { useTranslation } from "react-i18next";
import {
  getSettings,
  getPermissionRules,
  setPermissionRules,
  setTerminalShell as persistTerminalShell,
  getPermissionPresetVisibility,
  setPermissionPresetVisibility,
  setExecutionFeatures,
  setOutputStyle as persistOutputStyle,
  listTrustedProjects,
  setProjectTrust,
} from "../../api";
import type { OutputStyle } from "../../api";
import type { ExecutionFeatures } from "../../types";

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

function SegmentedControl({ options, value, onChange }: { options: { label: string, value: string }[], value: string, onChange: (val: string) => void }) {
  return (
    <div className="flex items-center bg-gray-100 p-0.5 rounded-lg border border-border-theme">
      {options.map((opt) => (
        <div 
          key={opt.value}
          onClick={() => onChange(opt.value)}
          className={`px-3 py-1 text-[12px] font-medium cursor-pointer transition-all rounded-md ${value === opt.value ? 'bg-white shadow-[0_1px_2px_rgb(0,0,0,0.1)] text-text-base' : 'text-text-secondary hover:text-text-base'}`}
        >
          {opt.label}
        </div>
      ))}
    </div>
  );
}

function DynamicIcon({ 
  opt, 
  className 
}: { 
  opt: { icon: string, isFab?: boolean, iconColor: string, logoUrl?: string, title: string }, 
  className: string 
}) {
  const [imgError, setImgError] = useState(false);

  if (opt.logoUrl && !imgError) {
    return (
      <img 
        src={opt.logoUrl} 
        alt={opt.title} 
        className={`w-[14px] h-[14px] object-contain ${className}`} 
        onError={() => setImgError(true)} 
      />
    );
  }

  return (
    <FontAwesomeIcon 
      icon={[opt.isFab ? "fab" : "fas", opt.icon as any]} 
      className={`text-[12px] ${opt.iconColor} ${className}`} 
    />
  );
}

function IconDropdown({ 
  options, 
  selectedTitle, 
  onChange 
}: { 
  options: { title: string, icon: string, isFab?: boolean, iconColor: string, logoUrl?: string }[], 
  selectedTitle: string, 
  onChange: (title: string) => void 
}) {
  const [isOpen, setIsOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(event.target as Node)) {
        setIsOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  const selectedOpt = options.find(o => o.title === selectedTitle) || options[0];

  return (
    <div className="relative" ref={dropdownRef}>
      <div 
        className="flex items-center bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg px-3 py-1.5 cursor-pointer transition-colors min-w-[150px] justify-between"
        onClick={() => setIsOpen(!isOpen)}
      >
        <div className="flex items-center">
          <DynamicIcon opt={selectedOpt} className="mr-2" />
          <span className="text-[12px] font-medium text-text-base">{selectedOpt.title}</span>
        </div>
        <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary ml-3" />
      </div>

      {isOpen && (
        <div className="absolute top-full right-0 mt-1 bg-white border border-border-theme rounded-xl shadow-[0_4px_20px_rgb(0,0,0,0.1)] z-20 py-2 w-[220px] max-h-[300px] overflow-y-auto">
          {options.map((opt) => (
            <div 
              key={opt.title}
              className="px-4 py-2 hover:bg-gray-50 cursor-pointer flex items-center justify-between"
              onClick={() => {
                onChange(opt.title);
                setIsOpen(false);
              }}
            >
              <div className="flex items-center">
                <DynamicIcon opt={opt} className="mr-3" />
                <span className="text-[13px] font-medium text-text-base">{opt.title}</span>
              </div>
              <div className="w-4 flex justify-end">
                {selectedTitle === opt.title && (
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

function ComplexDropdown({ 
  options, 
  selectedTitle, 
  onChange,
  width = "w-[200px]",
  dropdownWidth = "w-[320px]"
}: { 
  options: { title: string, description?: string }[], 
  selectedTitle: string, 
  onChange: (title: string) => void,
  width?: string,
  dropdownWidth?: string
}) {
  const [isOpen, setIsOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(event.target as Node)) {
        setIsOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  return (
    <div className="relative" ref={dropdownRef}>
      <div 
        className={`flex items-center bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg px-3 py-1.5 cursor-pointer transition-colors ${width} justify-between`}
        onClick={() => setIsOpen(!isOpen)}
      >
        <span className="text-[12px] font-medium text-text-base">{selectedTitle}</span>
        <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary ml-3" />
      </div>

      {isOpen && (
        <div className={`absolute top-full right-0 mt-1 bg-white border border-border-theme rounded-xl shadow-[0_4px_20px_rgb(0,0,0,0.1)] z-20 py-2 ${dropdownWidth}`}>
          {options.map((opt) => (
            <div 
              key={opt.title}
              className="px-4 py-2 hover:bg-gray-50 cursor-pointer flex items-center justify-between"
              onClick={() => {
                onChange(opt.title);
                setIsOpen(false);
              }}
            >
              <div className="flex-1 pr-4">
                <div className={`text-[13px] text-text-base ${opt.description ? 'font-medium mb-0.5' : ''}`}>{opt.title}</div>
                {opt.description && (
                  <div className="text-[12px] text-text-secondary leading-snug">{opt.description}</div>
                )}
              </div>
              <div className="w-4 flex justify-end">
                {selectedTitle === opt.title && (
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

function SearchableDropdown({ 
  options, 
  selectedTitle, 
  onChange 
}: { 
  options: { title: string }[], 
  selectedTitle: string, 
  onChange: (title: string) => void 
}) {
  const [isOpen, setIsOpen] = useState(false);
  const [search, setSearch] = useState("");
  const dropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(event.target as Node)) {
        setIsOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  const filteredOptions = options.filter(opt => opt.title.toLowerCase().includes(search.toLowerCase()));

  return (
    <div className="relative" ref={dropdownRef}>
      <div 
        className="flex items-center bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg px-3 py-1.5 cursor-pointer transition-colors w-[180px] justify-between"
        onClick={() => {
          setIsOpen(!isOpen);
          setSearch("");
        }}
      >
        <span className="text-[12px] font-medium text-text-base truncate pr-2">{selectedTitle}</span>
        <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary flex-shrink-0 ml-3" />
      </div>

      {isOpen && (
        <div className="absolute top-full right-0 mt-1 bg-white border border-border-theme rounded-xl shadow-[0_4px_20px_rgb(0,0,0,0.1)] z-20 flex flex-col w-[260px] max-h-[320px] overflow-hidden">
          <div className="p-2 border-b border-border-theme">
            <div className="flex items-center bg-gray-50 border border-border-theme rounded-md px-2.5 py-1.5">
              <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className="text-text-secondary text-[12px] mr-2" />
              <input 
                type="text" 
                placeholder="搜索语言" 
                className="bg-transparent border-none outline-none text-[12px] w-full placeholder:text-text-secondary"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                autoFocus
              />
            </div>
          </div>
          <div className="overflow-y-auto py-1">
            {filteredOptions.map((opt) => (
              <div 
                key={opt.title}
                className="px-4 py-2 hover:bg-gray-50 cursor-pointer flex items-center justify-between"
                onClick={() => {
                  onChange(opt.title);
                  setIsOpen(false);
                }}
              >
                <span className="text-[13px] text-text-base">{opt.title}</span>
                <div className="w-4 flex justify-end">
                  {selectedTitle === opt.title && (
                    <FontAwesomeIcon icon={["fas", "check"]} className="text-[12px] text-text-base" />
                  )}
                </div>
              </div>
            ))}
            {filteredOptions.length === 0 && (
              <div className="px-4 py-3 text-[12px] text-text-secondary text-center">
                未找到相关语言
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

const EDITOR_OPTIONS = [
  { title: "VS Code", icon: "code", iconColor: "text-blue-500", logoUrl: "https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/vscode/vscode-original.svg" },
  { title: "Visual Studio", icon: "laptop-code", iconColor: "text-purple-600", logoUrl: "https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/visualstudio/visualstudio-original.svg" },
  { title: "Cursor", icon: "pen-nib", iconColor: "text-gray-800", logoUrl: "https://avatars.githubusercontent.com/u/127768225?s=200&v=4" },
  { title: "Zed", icon: "bolt", iconColor: "text-yellow-600", logoUrl: "https://avatars.githubusercontent.com/u/108922485?s=200&v=4" },
  { title: "Antigravity", icon: "rocket", iconColor: "text-orange-500", logoUrl: "/logo.png" },
  { title: "Default app", icon: "window-maximize", iconColor: "text-gray-500" },
  { title: "File Explorer", icon: "folder", iconColor: "text-yellow-500", logoUrl: "https://upload.wikimedia.org/wikipedia/commons/2/20/File_Explorer_Icon_%28Windows_11%29.svg" },
  { title: "Terminal", icon: "terminal", iconColor: "text-gray-800", logoUrl: "https://upload.wikimedia.org/wikipedia/commons/4/4b/Windows_Terminal_logo.svg" },
  { title: "WSL", icon: "linux", isFab: true, iconColor: "text-gray-800", logoUrl: "https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/linux/linux-original.svg" }
];

const AGENT_ENV_OPTIONS = [
  { title: "Windows 原生", description: "直接在 Windows 中运行智能体" },
  { title: "适用于 Linux 的 Windows 子系统", description: "在 WSL 中运行智能体" }
];

const SHELL_OPTIONS = [
  { title: "PowerShell" },
  { title: "Command Prompt" },
  { title: "Git Bash" },
  { title: "WSL" }
];

function shellKeyToTitle(shell: "powershell" | "command_prompt" | "git_bash" | "wsl"): string {
  switch (shell) {
    case "command_prompt":
      return "Command Prompt";
    case "git_bash":
      return "Git Bash";
    case "wsl":
      return "WSL";
    case "powershell":
    default:
      return "PowerShell";
  }
}

function shellTitleToKey(title: string): "powershell" | "command_prompt" | "git_bash" | "wsl" {
  switch (title) {
    case "Command Prompt":
      return "command_prompt";
    case "Git Bash":
      return "git_bash";
    case "WSL":
      return "wsl";
    case "PowerShell":
    default:
      return "powershell";
  }
}

const LANGUAGE_OPTIONS = [
  { title: "自动检测" },
  { title: "阿尔巴尼亚语 (阿尔巴尼亚)" },
  { title: "冰岛语 (冰岛)" },
  { title: "格鲁吉亚语 (格鲁吉亚)" },
  { title: "马其顿语 (北马其顿)" },
  { title: "蒙古语" },
  { title: "缅甸语 (缅甸)" },
  { title: "日本语 (日本) · 日语 (日本)" },
  { title: "索马里语 (索马里)" },
  { title: "亚美尼亚语 (亚美尼亚)" },
  { title: "中文 (台湾) · 中文 (台湾)" },
  { title: "中文 (香港)" },
  { title: "中文 (简体)" },
  { title: "English (US)" }
];

export function GeneralSettings() {
  const { t, i18n } = useTranslation();
  
  const [workMode, setWorkMode] = useState<"code" | "daily">(() => (localStorage.getItem("workMode") as any) || "code");

  const handleWorkModeChange = (mode: "code" | "daily") => {
    setWorkMode(mode);
    localStorage.setItem("workMode", mode);
  };
  const [permDefault, setPermDefault] = useState(true);
  const [permAuto, setPermAuto] = useState(true);
  const [permFull, setPermFull] = useState(true);

  useEffect(() => {
    getPermissionPresetVisibility()
      .then((v) => {
        setPermDefault(v.default_enabled);
        setPermAuto(v.auto_review_enabled);
        setPermFull(v.full_access_enabled);
      })
      .catch(() => {});
  }, []);

  const togglePerm = (type: "default" | "auto" | "full", val: boolean) => {
    const next = {
      default_enabled: type === "default" ? val : permDefault,
      auto_review_enabled: type === "auto" ? val : permAuto,
      full_access_enabled: type === "full" ? val : permFull,
    };
    if (!next.default_enabled && !next.auto_review_enabled && !next.full_access_enabled) return;
    if (type === "default") setPermDefault(val);
    if (type === "auto") setPermAuto(val);
    if (type === "full") setPermFull(val);
    setPermissionPresetVisibility(next).catch(() => {});
    window.dispatchEvent(new Event("approvalMenuChanged"));
  };

  // Declarative permission rules (allow/ask/deny patterns, one per line).
  const [rulesAllow, setRulesAllow] = useState("");
  const [rulesAsk, setRulesAsk] = useState("");
  const [rulesDeny, setRulesDeny] = useState("");
  useEffect(() => {
    getPermissionRules()
      .then((r) => {
        setRulesAllow(r.allow.join("\n"));
        setRulesAsk(r.ask.join("\n"));
        setRulesDeny(r.deny.join("\n"));
      })
      .catch(() => {});
  }, []);
  const toLines = (s: string) =>
    s.split("\n").map((l) => l.trim()).filter((l) => l !== "");
  const saveRules = () => {
    setPermissionRules({
      allow: toLines(rulesAllow),
      ask: toLines(rulesAsk),
      deny: toLines(rulesDeny),
    }).catch(() => {});
  };

  const [editorTarget, setEditorTarget] = useState("VS Code");
  const [agentEnv, setAgentEnv] = useState("Windows 原生");
  const [terminalShell, setTerminalShell] = useState("PowerShell");
  useEffect(() => {
    getSettings()
      .then((view) => {
        if (view?.terminal_shell) {
          setTerminalShell(shellKeyToTitle(view.terminal_shell));
        }
        if (view?.execution_features) {
          setExecFeatures(view.execution_features);
        }
        if (view?.output_style) {
          setOutputStyle(view.output_style as OutputStyle);
        }
      })
      .catch(() => {});
  }, []);

  // Built-in output style (§7.1): default / explanatory / learning. Switching
  // persists via set_output_style and takes effect on the next run's system
  // prompt (a stable, cacheable style block).
  const [outputStyle, setOutputStyle] = useState<OutputStyle>("default");
  const changeOutputStyle = (next: OutputStyle) => {
    const prev = outputStyle;
    setOutputStyle(next);
    persistOutputStyle(next).catch((e) => {
      console.error("set_output_style failed:", e);
      setOutputStyle(prev);
    });
  };

  // Opt-in advanced execution safeguards (§2.2/§2.3/§6.1/§6.2). All default
  // OFF; toggling persists via set_execution_features. The matching
  // DEEPAGENT_* env var force-enables regardless of this switch.
  const [execFeatures, setExecFeatures] = useState<ExecutionFeatures>({
    stall_detector: false,
    command_guard: false,
    project_trust: false,
    adversarial_verify: false,
  });
  const toggleExecFeature = (key: keyof ExecutionFeatures) => {
    const next = { ...execFeatures, [key]: !execFeatures[key] };
    setExecFeatures(next);
    setExecutionFeatures(next).catch((e) => {
      console.error("set_execution_features failed:", e);
      // Roll back the optimistic toggle on failure.
      setExecFeatures(execFeatures);
    });
  };

  // §6.2 trusted-project revoke list: the explicit grants the user made via the
  // TrustDialog. Revoking removes the grant (descendants lose implicit trust).
  const [trustedProjects, setTrustedProjects] = useState<string[]>([]);
  useEffect(() => {
    listTrustedProjects().then(setTrustedProjects).catch(() => {});
  }, []);
  const revokeTrust = (path: string) => {
    setProjectTrust(path, false)
      .then(() => listTrustedProjects())
      .then(setTrustedProjects)
      .catch((e) => console.error("revoke trust failed:", e));
  };
  
  const mapLangToTitle = (lng: string) => {
    switch (lng) {
      case "sq": return "阿尔巴尼亚语 (阿尔巴尼亚)";
      case "is": return "冰岛语 (冰岛)";
      case "ka": return "格鲁吉亚语 (格鲁吉亚)";
      case "mk": return "马其顿语 (北马其顿)";
      case "mn": return "蒙古语";
      case "my": return "缅甸语 (缅甸)";
      case "ja": return "日本语 (日本) · 日语 (日本)";
      case "so": return "索马里语 (索马里)";
      case "hy": return "亚美尼亚语 (亚美尼亚)";
      case "zh-TW": return "中文 (台湾) · 中文 (台湾)";
      case "zh-HK": return "中文 (香港)";
      case "zh": return "中文 (简体)";
      case "en": return "English (US)";
      default: return "自动检测";
    }
  };
  
  const mapTitleToLang = (title: string) => {
    switch (title) {
      case "阿尔巴尼亚语 (阿尔巴尼亚)": return "sq";
      case "冰岛语 (冰岛)": return "is";
      case "格鲁吉亚语 (格鲁吉亚)": return "ka";
      case "马其顿语 (北马其顿)": return "mk";
      case "蒙古语": return "mn";
      case "缅甸语 (缅甸)": return "my";
      case "日本语 (日本) · 日语 (日本)": return "ja";
      case "索马里语 (索马里)": return "so";
      case "亚美尼亚语 (亚美尼亚)": return "hy";
      case "中文 (台湾) · 中文 (台湾)": return "zh-TW";
      case "中文 (香港)": return "zh-HK";
      case "中文 (简体)": return "zh";
      case "English (US)": return "en";
      default: return "zh"; // Fallback
    }
  };

  const [appLanguage, setAppLanguage] = useState(mapLangToTitle(i18n.language));
  const [notificationSetting, setNotificationSetting] = useState(t("settings.general.notifications.unfocused"));
  
  const [longTextPrompt, setLongTextPrompt] = useState(false);
  const [suggestPrompt, setSuggestPrompt] = useState(true);
  const [noProjectChat, setNoProjectChat] = useState(false);
  const [notifyPermission, setNotifyPermission] = useState(true);
  const [notifyIssue, setNotifyIssue] = useState(true);
  
  const [followBehavior, setFollowBehavior] = useState<"queue" | "guide">("queue");
  const [codeReviewView, setCodeReviewView] = useState<"inline" | "split">("inline");

  const handleLanguageChange = (title: string) => {
    setAppLanguage(title);
    const lng = mapTitleToLang(title);
    i18n.changeLanguage(lng);
    localStorage.setItem("appLanguage", lng);
  };

  return (
    <>
      <h1 className="text-2xl font-semibold text-text-base mb-10">{t("settings.general.title")}</h1>

      {/* Section: 工作模式 */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-1">{t("settings.general.workMode.title")}</h2>
        <p className="text-[13px] text-text-secondary mb-4">{t("settings.general.workMode.desc")}</p>
        <div className="flex space-x-4">
          <div 
            className={`flex-1 rounded-xl p-4 cursor-pointer transition-all border-2 relative overflow-hidden ${workMode === 'code' ? 'border-blue-100 bg-gray-50' : 'border-border-theme bg-white hover:bg-gray-50'}`}
            onClick={() => handleWorkModeChange('code')}
          >
            <div className="flex items-start">
              <div className="w-8 h-8 rounded-lg bg-white border border-border-theme flex items-center justify-center mr-3 shadow-sm flex-shrink-0">
                <FontAwesomeIcon icon={["fas", "terminal"]} className="text-text-base text-[13px]" />
              </div>
              <div className="flex-1">
                <div className="text-[14px] font-medium text-text-base mb-0.5">{t("settings.general.workMode.code")}</div>
                <div className="text-[12px] text-text-secondary">{t("settings.general.workMode.codeDesc")}</div>
              </div>
              <div className="ml-2 pt-1">
                <div className={`w-[18px] h-[18px] rounded-full border-[5px] ${workMode === 'code' ? 'border-blue-500 bg-white' : 'border-gray-200'}`} />
              </div>
            </div>
          </div>

          <div 
            className={`flex-1 rounded-xl p-4 cursor-pointer transition-all border-2 relative overflow-hidden ${workMode === 'daily' ? 'border-blue-100 bg-gray-50' : 'border-border-theme bg-white hover:bg-gray-50'}`}
            onClick={() => handleWorkModeChange('daily')}
          >
            <div className="flex items-start">
              <div className="w-8 h-8 rounded-lg bg-white border border-border-theme flex items-center justify-center mr-3 shadow-sm flex-shrink-0">
                <FontAwesomeIcon icon={["far", "comments"]} className="text-text-base text-[13px]" />
              </div>
              <div className="flex-1">
                <div className="text-[14px] font-medium text-text-base mb-0.5">{t("settings.general.workMode.daily")}</div>
                <div className="text-[12px] text-text-secondary">{t("settings.general.workMode.dailyDesc")}</div>
              </div>
              <div className="ml-2 pt-1">
                <div className={`w-[18px] h-[18px] rounded-full border-[5px] ${workMode === 'daily' ? 'border-blue-500 bg-white' : 'border-gray-200'}`} />
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* Section: 权限 */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-4">{t("settings.general.permissions.title")}</h2>
        <div className="border border-border-theme rounded-xl overflow-hidden shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          <div className="flex items-start justify-between p-4 bg-white border-b border-border-theme">
            <div className="pr-8">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.permissions.default")}</div>
              <div className="text-[12px] text-text-secondary leading-relaxed">
                {t("settings.general.permissions.defaultDesc")}
              </div>
            </div>
            <div className="pt-1">
              <ToggleSwitch checked={permDefault} onChange={() => togglePerm("default", !permDefault)} />
            </div>
          </div>

          <div className="flex items-start justify-between p-4 bg-white border-b border-border-theme">
            <div className="pr-8">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.permissions.auto")}</div>
              <div className="text-[12px] text-text-secondary leading-relaxed">
                {t("settings.general.permissions.autoDesc")}
              </div>
            </div>
            <div className="pt-1">
              <ToggleSwitch checked={permAuto} onChange={() => togglePerm("auto", !permAuto)} />
            </div>
          </div>

          <div className="flex items-start justify-between p-4 bg-white">
            <div className="pr-8">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.permissions.full")}</div>
              <div className="text-[12px] text-text-secondary leading-relaxed">
                {t("settings.general.permissions.fullDesc")}
              </div>
            </div>
            <div className="pt-1">
              <ToggleSwitch checked={permFull} onChange={() => togglePerm("full", !permFull)} />
            </div>
          </div>
        </div>

        {/* 声明式权限规则（allow / ask / deny 模式，每行一条；支持 Bash(git:*)） */}
        <div className="mt-6 border border-border-theme rounded-xl bg-white p-4">
          <div className="text-[14px] font-medium text-text-base mb-1">工具权限规则</div>
          <div className="text-[12px] text-text-secondary mb-4 leading-relaxed">
            声明式规则，每行一条。支持 <code className="bg-gray-100 px-1 rounded">工具名</code>（如 <code className="bg-gray-100 px-1 rounded">WebFetch</code>）或 <code className="bg-gray-100 px-1 rounded">Bash(git:*)</code> 命令前缀。优先级：拒绝 &gt; 询问 &gt; 允许。
          </div>
          <div className="grid grid-cols-3 gap-3">
            <div>
              <div className="text-[12px] font-medium text-green-600 mb-1">允许 (allow)</div>
              <textarea
                value={rulesAllow}
                onChange={(e) => setRulesAllow(e.target.value)}
                onBlur={saveRules}
                rows={5}
                placeholder={"Bash(git:*)\nread_file"}
                className="w-full border border-border-theme rounded-lg p-2 text-[12px] font-mono resize-none focus:outline-none focus:border-blue-500 bg-white"
              />
            </div>
            <div>
              <div className="text-[12px] font-medium text-amber-600 mb-1">询问 (ask)</div>
              <textarea
                value={rulesAsk}
                onChange={(e) => setRulesAsk(e.target.value)}
                onBlur={saveRules}
                rows={5}
                placeholder={"Bash(rm:*)\nwrite_file"}
                className="w-full border border-border-theme rounded-lg p-2 text-[12px] font-mono resize-none focus:outline-none focus:border-blue-500 bg-white"
              />
            </div>
            <div>
              <div className="text-[12px] font-medium text-red-600 mb-1">拒绝 (deny)</div>
              <textarea
                value={rulesDeny}
                onChange={(e) => setRulesDeny(e.target.value)}
                onBlur={saveRules}
                rows={5}
                placeholder={"WebSearch\nBash(sudo:*)"}
                className="w-full border border-border-theme rounded-lg p-2 text-[12px] font-mono resize-none focus:outline-none focus:border-blue-500 bg-white"
              />
            </div>
          </div>
        </div>
      </div>

      {/* Section: 高级执行防护（§2.2/§2.3/§6.1/§6.2，默认均关） */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-1">高级执行防护</h2>
        <div className="text-[12px] text-text-secondary mb-4 leading-relaxed">
          选入式安全岗哨，默认关闭。均为“宁可漏过不可误杀”的建议性机制（fail-open，不会阻断 run）。开启后会在相应时机额外调用一次轻量模型判定。
        </div>
        <div className="border border-border-theme rounded-xl overflow-hidden shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          <div className="flex items-start justify-between p-4 bg-white border-b border-border-theme">
            <div className="pr-8">
              <div className="text-[14px] font-medium text-text-base mb-1">停滞/假完成检测</div>
              <div className="text-[12px] text-text-secondary leading-relaxed">
                终答时审查是否“只声称完成但无实际证据”，命中时注入一次建议提醒让模型继续（§2.3）。
              </div>
            </div>
            <div className="pt-1">
              <ToggleSwitch
                checked={execFeatures.stall_detector}
                onChange={() => toggleExecFeature("stall_detector")}
              />
            </div>
          </div>

          <div className="flex items-start justify-between p-4 bg-white border-b border-border-theme">
            <div className="pr-8">
              <div className="text-[14px] font-medium text-text-base mb-1">命令注入检测（LLM）</div>
              <div className="text-[12px] text-text-secondary leading-relaxed">
                对结构可疑的 shell 命令做一次模型复审，疑似注入/渗出时升级为人工审批（§6.1）。
              </div>
            </div>
            <div className="pt-1">
              <ToggleSwitch
                checked={execFeatures.command_guard}
                onChange={() => toggleExecFeature("command_guard")}
              />
            </div>
          </div>

          <div className="flex items-start justify-between p-4 bg-white border-b border-border-theme">
            <div className="pr-8">
              <div className="text-[14px] font-medium text-text-base mb-1">项目信任网关</div>
              <div className="text-[12px] text-text-secondary leading-relaxed">
                未信任的项目目录下，即使是白名单命令也将 bash/shell 升级为人工审批（§6.2）。
              </div>
            </div>
            <div className="pt-1">
              <ToggleSwitch
                checked={execFeatures.project_trust}
                onChange={() => toggleExecFeature("project_trust")}
              />
            </div>
          </div>

          <div className="flex items-start justify-between p-4 bg-white">
            <div className="pr-8">
              <div className="text-[14px] font-medium text-text-base mb-1">对抗式目标验证</div>
              <div className="text-[12px] text-text-secondary leading-relaxed">
                任务改动文件并声称完成后，由一个只读“怀疑者面板”审核目标是否真正达成（多数驳回，§2.2）。
              </div>
            </div>
            <div className="pt-1">
              <ToggleSwitch
                checked={execFeatures.adversarial_verify}
                onChange={() => toggleExecFeature("adversarial_verify")}
              />
            </div>
          </div>
        </div>

        {/* §6.2 已信任项目列表 + 撤销 */}
        <div className="mt-4 border border-border-theme rounded-xl bg-white p-4">
          <div className="text-[14px] font-medium text-text-base mb-1">已信任的项目</div>
          <div className="text-[12px] text-text-secondary mb-3 leading-relaxed">
            你通过信任弹框授予信任的项目目录（子目录自动继承，不单列）。撤销后，开启信任网关时该目录将重新需要确认。
          </div>
          {trustedProjects.length === 0 ? (
            <div className="text-[12px] text-text-secondary italic">暂无已信任的项目。</div>
          ) : (
            <div className="flex flex-col gap-2">
              {trustedProjects.map((path) => (
                <div
                  key={path}
                  className="flex items-center justify-between gap-3 rounded-lg border border-border-theme px-3 py-2"
                >
                  <span className="text-[12px] font-mono text-text-base break-all">{path}</span>
                  <button
                    type="button"
                    onClick={() => revokeTrust(path)}
                    className="flex-shrink-0 px-2.5 py-1 text-[12px] rounded-md border border-border-theme text-red-600 hover:bg-red-50 transition-colors"
                  >
                    撤销
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      {/* Section: 输出风格（§7.1 output styles） */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-1">输出风格</h2>
        <div className="text-[12px] text-text-secondary mb-4 leading-relaxed">
          内置系统提示风格，切换后自下一次会话生效（注入稳定可缓存的风格块，§7.1）。
        </div>
        <div className="border border-border-theme rounded-xl bg-white p-4 flex items-center justify-between">
          <div className="pr-8">
            <div className="text-[14px] font-medium text-text-base mb-1">回复风格</div>
            <div className="text-[12px] text-text-secondary leading-relaxed">
              默认：由基座提示词控制；解释型：边交付边补“为何”洞见；学习型：教学式讲解概念与思路。
            </div>
          </div>
          <SegmentedControl
            options={[
              { label: "默认", value: "default" },
              { label: "解释型", value: "explanatory" },
              { label: "学习型", value: "learning" },
            ]}
            value={outputStyle}
            onChange={(val) => changeOutputStyle(val as OutputStyle)}
          />
        </div>
      </div>

      {/* Section: 常规 */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-4">{t("settings.general.generalSection.title")}</h2>
        <div className="border border-border-theme rounded-xl overflow-hidden bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.generalSection.defaultTarget")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.generalSection.defaultTargetDesc")}</div>
            </div>
            <IconDropdown 
              options={EDITOR_OPTIONS} 
              selectedTitle={editorTarget} 
              onChange={setEditorTarget} 
            />
          </div>

          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.generalSection.agentEnv")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.generalSection.agentEnvDesc")}</div>
            </div>
            <ComplexDropdown 
              options={AGENT_ENV_OPTIONS} 
              selectedTitle={agentEnv} 
              onChange={setAgentEnv} 
            />
          </div>

          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.generalSection.shell")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.generalSection.shellDesc")}</div>
            </div>
            <ComplexDropdown 
              options={SHELL_OPTIONS} 
              selectedTitle={terminalShell} 
              onChange={(title) => {
                setTerminalShell(title);
                persistTerminalShell(shellTitleToKey(title)).catch(() => {});
              }} 
              dropdownWidth="w-[200px]"
            />
          </div>

          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.generalSection.language")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.generalSection.languageDesc")}</div>
            </div>
            <SearchableDropdown 
              options={LANGUAGE_OPTIONS} 
              selectedTitle={appLanguage} 
              onChange={handleLanguageChange} 
            />
          </div>

          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.generalSection.longText")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.generalSection.longTextDesc")}</div>
            </div>
            <ToggleSwitch checked={longTextPrompt} onChange={() => setLongTextPrompt(!longTextPrompt)} />
          </div>

          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div className="pr-4">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.generalSection.followUp")}</div>
              <div className="text-[12px] text-text-secondary leading-relaxed">
                {t("settings.general.generalSection.followUpDesc")}
              </div>
            </div>
            <SegmentedControl 
              options={[{label: t("settings.general.generalSection.queue"), value: 'queue'}, {label: t("settings.general.generalSection.guide"), value: 'guide'}]} 
              value={followBehavior} 
              onChange={(val) => setFollowBehavior(val as "queue" | "guide")} 
            />
          </div>

          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.generalSection.codeReview")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.generalSection.codeReviewDesc")}</div>
            </div>
            <SegmentedControl 
              options={[{label: t("settings.general.generalSection.inline"), value: 'inline'}, {label: t("settings.general.generalSection.split"), value: 'split'}]} 
              value={codeReviewView} 
              onChange={(val) => setCodeReviewView(val as "inline" | "split")} 
            />
          </div>

          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.generalSection.suggestions")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.generalSection.suggestionsDesc")}</div>
            </div>
            <ToggleSwitch checked={suggestPrompt} onChange={() => setSuggestPrompt(!suggestPrompt)} />
          </div>

          <div className="flex items-center justify-between p-4">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.generalSection.import")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.generalSection.importDesc")}</div>
            </div>
            <button className="px-4 py-1.5 bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg text-[12px] font-medium text-text-base transition-colors ml-4">
              {t("settings.general.generalSection.importBtn")}
            </button>
          </div>
        </div>
      </div>

      {/* Section: 弹出窗口 */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-4">{t("settings.general.popup.title")}</h2>
        <div className="border border-border-theme rounded-xl overflow-hidden shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          <div className="flex items-center justify-between p-4 bg-white border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.popup.shortcut")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.popup.shortcutDesc")}</div>
            </div>
            <div className="flex items-center">
              <span className="text-[12px] text-text-secondary mr-3">{t("settings.general.popup.disabled")}</span>
              <button className="px-3 py-1.5 bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg text-[12px] font-medium text-text-base transition-colors">{t("settings.general.popup.set")}</button>
            </div>
          </div>
          <div className="flex items-center justify-between p-4 bg-white">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.popup.noProject")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.popup.noProjectDesc")}</div>
            </div>
            <div>
              <ToggleSwitch checked={noProjectChat} onChange={() => setNoProjectChat(!noProjectChat)} />
            </div>
          </div>
        </div>
      </div>

      {/* Section: 听写 */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-4">{t("settings.general.dictation.title")}</h2>
        <div className="border border-border-theme rounded-xl overflow-hidden shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          <div className="flex items-center justify-between p-4 bg-white border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.dictation.hold")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.dictation.holdDesc")}</div>
            </div>
            <div className="flex items-center">
              <span className="text-[12px] text-text-secondary mr-3">{t("settings.general.dictation.off")}</span>
              <button className="px-3 py-1.5 bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg text-[12px] font-medium text-text-base transition-colors">{t("settings.general.dictation.set")}</button>
            </div>
          </div>
          <div className="flex items-center justify-between p-4 bg-white border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.dictation.toggle")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.dictation.toggleDesc")}</div>
            </div>
            <div className="flex items-center">
              <span className="text-[12px] text-text-secondary mr-3">{t("settings.general.dictation.off")}</span>
              <button className="px-3 py-1.5 bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg text-[12px] font-medium text-text-base transition-colors">{t("settings.general.dictation.set")}</button>
            </div>
          </div>
          <div className="flex items-center justify-between p-4 bg-white border-b border-border-theme cursor-pointer hover:bg-gray-50 transition-colors">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.dictation.dictionary")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.dictation.dictionaryDesc")}</div>
            </div>
            <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary ml-3" />
          </div>
          <div className="p-4 bg-white">
            <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.dictation.recent")}</div>
            <div className="text-[12px] text-text-secondary">{t("settings.general.dictation.recentDesc")}</div>
          </div>
        </div>
      </div>

      {/* Section: 通知 */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-4">{t("settings.general.notifications.title")}</h2>
        <div className="border border-border-theme rounded-xl overflow-hidden shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          <div className="flex items-center justify-between p-4 bg-white border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.notifications.turnDone")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.notifications.turnDoneDesc")}</div>
            </div>
            <ComplexDropdown 
              options={[{ title: t("settings.general.notifications.always") }, { title: t("settings.general.notifications.unfocused") }, { title: t("settings.general.notifications.never") }]} 
              selectedTitle={notificationSetting} 
              onChange={setNotificationSetting} 
              dropdownWidth="w-[200px]"
            />
          </div>
          <div className="flex items-center justify-between p-4 bg-white border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.notifications.permission")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.notifications.permissionDesc")}</div>
            </div>
            <div>
              <ToggleSwitch checked={notifyPermission} onChange={() => setNotifyPermission(!notifyPermission)} />
            </div>
          </div>
          <div className="flex items-center justify-between p-4 bg-white">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.general.notifications.issue")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.general.notifications.issueDesc")}</div>
            </div>
            <div>
              <ToggleSwitch checked={notifyIssue} onChange={() => setNotifyIssue(!notifyIssue)} />
            </div>
          </div>
        </div>
      </div>
    </>
  );
}
