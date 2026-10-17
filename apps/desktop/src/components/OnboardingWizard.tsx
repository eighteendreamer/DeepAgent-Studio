import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, Eye, FolderOpen, Globe, LoaderCircle, Lock, LockOpen, MessagesSquare, Monitor, Moon, Shield, SlidersHorizontal, Sun, Terminal } from "lucide-react";
import { getWelcomeName, initializeProject, isTauri, setSandboxMode, setWelcomeName as persistWelcomeName, type SandboxMode } from "../api";
import { message } from "./message";
import { useTranslation } from "react-i18next";
import { useTheme, type ThemeMode } from "../hooks/useTheme";
import { startOnboardingEntrance, type OnboardingEntrance } from "./onboardingEntrance";
import { Button } from "./shadcn/button";

interface Props {
  onComplete: () => void;
}

const SUPPORTED_LANGUAGES = [
  { id: "zh", label: "中文 (简体)" },
  { id: "en", label: "English" },
  { id: "zh-TW", label: "中文 (繁體 - 台湾)" },
  { id: "zh-HK", label: "中文 (繁體 - 香港)" },
  { id: "ja", label: "日本語" },
  { id: "sq", label: "Shqip (Albanian)" },
  { id: "is", label: "Íslenska (Icelandic)" },
  { id: "ka", label: "ქართული (Georgian)" },
  { id: "mk", label: "Македонски (Macedonian)" },
  { id: "mn", label: "Монгол (Mongolian)" },
  { id: "my", label: "မြန်မာ (Burmese)" },
  { id: "so", label: "Soomaali (Somali)" },
  { id: "hy", label: "Հայերեն (Armenian)" },
];

export function OnboardingWizard({ onComplete }: Props) {
  const [step, setStep] = useState<0 | 1 | 2 | 3 | 4>(0);
  const [apiKey, setApiKey] = useState("");
  const [isConnecting, setIsConnecting] = useState(false);
  const [isFinishing, setIsFinishing] = useState(false);
  const finishPendingRef = useRef(false);
  const entranceRef = useRef<OnboardingEntrance | null>(null);
  const overlayRef = useRef<HTMLDivElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const welcomeRef = useRef<HTMLDivElement>(null);
  const pointRef = useRef<HTMLDivElement>(null);
  const leftDoorRef = useRef<HTMLDivElement>(null);
  const rightDoorRef = useRef<HTMLDivElement>(null);
  const leftSeamRef = useRef<HTMLDivElement>(null);
  const rightSeamRef = useRef<HTMLDivElement>(null);

  useEffect(() => () => {
    finishPendingRef.current = false;
    entranceRef.current?.cancel();
  }, []);
  
  const { t, i18n } = useTranslation();
  const { activeIsDark, switchTheme } = useTheme();
  
  const [theme, setTheme] = useState<"light" | "dark" | "system">("system");
  const [language, setLanguage] = useState("zh");
  const [isLangDropdownOpen, setIsLangDropdownOpen] = useState(false);
  const [workMode, setWorkMode] = useState<"code" | "daily">("code");
  const [sandboxMode, setSandboxModeState] = useState<SandboxMode>("workspace_write");
  const [welcomeName, setWelcomeName] = useState(() => localStorage.getItem("userName")?.trim() || "");

  useEffect(() => {
    let active = true;
    const legacyName = localStorage.getItem("userName")?.trim() || "";

    getWelcomeName()
      .then(async (storedName) => {
        if (!active) return;
        const databaseName = storedName.trim();
        if (databaseName) {
          setWelcomeName(databaseName);
          if (isTauri()) localStorage.removeItem("userName");
          return;
        }

        if (legacyName) {
          try {
            const migratedName = await persistWelcomeName(legacyName);
            if (!active) return;
            setWelcomeName(migratedName);
            if (isTauri()) localStorage.removeItem("userName");
          } catch (error) {
            console.error("failed to migrate welcome name:", error);
          }
        }
      })
      .catch((error) => console.error("failed to load welcome name:", error));

    return () => {
      active = false;
    };
  }, []);

  const handleConnect = async () => {
    const key = apiKey.trim();
    if (!key || isConnecting) return;

    // The key is validated by the Rust backend (it runs DeepSeek model
    // discovery). That backend only exists inside the desktop app — a plain
    // browser preview has no way to validate, so we refuse to proceed there.
    if (!isTauri()) {
      message.error("连接验证需要在桌面应用中进行（请运行桌面客户端）");
      return;
    }

    setIsConnecting(true);
    try {
      // Invalid key → backend returns a 401 error → this throws → we stay on
      // step 0. Only a valid key (stored as encrypted SQLite ciphertext) advances.
      await initializeProject(key);
      message.success("连接成功，API Key 已验证");
      setStep(1);
    } catch (e) {
      message.error("连接失败：API Key 无效或网络不可用");
      // Keep the detailed reason in the console for debugging.
      console.error("initialize_project failed:", e);
    } finally {
      setIsConnecting(false);
    }
  };

  const handleFinish = async () => {
    if (finishPendingRef.current) return;
    finishPendingRef.current = true;
    setIsFinishing(true);

    try {
      await setSandboxMode(sandboxMode);
      if (!finishPendingRef.current) return;
      // Theme is already persisted by ThemeProvider; retain the existing keys.
      localStorage.setItem("appLanguage", language);
      localStorage.setItem("workMode", workMode);
      localStorage.setItem("onboarding_complete", "true");
    } catch (error) {
      if (!finishPendingRef.current) return;
      console.error("onboarding settings save failed:", error);
      message.error("初始化设置保存失败，请重试");
      finishPendingRef.current = false;
      setIsFinishing(false);
      return;
    }

    try {
      const overlay = overlayRef.current;
      const card = cardRef.current;
      const welcome = welcomeRef.current;
      const point = pointRef.current;
      const leftDoor = leftDoorRef.current;
      const rightDoor = rightDoorRef.current;
      const leftSeam = leftSeamRef.current;
      const rightSeam = rightSeamRef.current;
      if (!overlay || !card || !welcome || !point || !leftDoor || !rightDoor || !leftSeam || !rightSeam) {
        throw new Error("onboarding entrance elements are unavailable");
      }
      entranceRef.current = startOnboardingEntrance({
        overlay, card, welcome, point,
        doors: [leftDoor, rightDoor], seams: [leftSeam, rightSeam],
      }, window.matchMedia("(prefers-reduced-motion: reduce)").matches);
      const completed = await entranceRef.current.finished;
      if (completed && finishPendingRef.current) onComplete();
    } catch (error) {
      if (!finishPendingRef.current) return;
      entranceRef.current?.cancel();
      entranceRef.current = null;
      console.error("onboarding entrance failed:", error);
      message.error("进入系统失败，请重试");
      finishPendingRef.current = false;
      setIsFinishing(false);
    }
  };

  const nextStep = () => setStep((s) => Math.min(s + 1, 4) as any);
  const prevStep = () => setStep((s) => Math.max(s - 1, 1) as any);

  // Custom theme classes based on user request
  const themeStyles = {
    primaryBg: "bg-text-base hover:opacity-90",
    primaryText: "text-bg-base",
    textBase: "text-text-base",
    textSecondary: "text-text-secondary",
    borderTheme: "border-border-theme",
    selectedBorder: "border-text-base",
    selectedBg: "bg-sidebar-bg",
  };

  // Keep the cinematic transition visible in both themes. The door surface is
  // a subtle tint over the base layer, while the seam uses the opposite neutral
  // contrast instead of a hard-coded white that disappears on light themes.
  const transitionColor = activeIsDark
    ? "rgba(255, 255, 255, 0.72)"
    : "rgba(17, 24, 39, 0.58)";
  const transitionShadow = activeIsDark
    ? "0 0 12px rgba(255, 255, 255, 0.32)"
    : "0 0 12px rgba(17, 24, 39, 0.18)";
  const doorSurface = activeIsDark
    ? "rgba(255, 255, 255, 0.06)"
    : "rgba(17, 24, 39, 0.055)";

  return (
    <div ref={overlayRef} className="absolute top-10 inset-x-0 bottom-0 z-50 flex items-center justify-center overflow-hidden">
      <div className="absolute inset-0 z-10 pointer-events-none" aria-hidden="true">
        <div ref={leftDoorRef} className="onboarding-door absolute inset-y-0 left-0 w-1/2 bg-bg-base">
          <div className="absolute inset-0" style={{ backgroundColor: doorSurface }} />
          <div className="absolute right-0 inset-y-0 flex items-center">
            <div ref={leftSeamRef} className="onboarding-door-line w-px" style={{ backgroundColor: transitionColor, boxShadow: transitionShadow }} />
          </div>
        </div>
        <div ref={rightDoorRef} className="onboarding-door absolute inset-y-0 right-0 w-1/2 bg-bg-base">
          <div className="absolute inset-0" style={{ backgroundColor: doorSurface }} />
          <div className="absolute left-0 inset-y-0 flex items-center">
            <div ref={rightSeamRef} className="onboarding-door-line w-px" style={{ backgroundColor: transitionColor, boxShadow: transitionShadow }} />
          </div>
        </div>
      </div>
      <div ref={pointRef} aria-hidden="true" className="onboarding-transition-dot absolute left-1/2 top-1/2 z-20 h-px w-px rounded-full" style={{ backgroundColor: transitionColor, boxShadow: transitionShadow }} />
      <div ref={welcomeRef} className={`onboarding-welcome-text absolute z-50 text-3xl font-bold tracking-wider ${themeStyles.textBase} pointer-events-none`}>
        {t("settings.personalize.greetingPrefix")}{welcomeName || t("settings.personalize.greetingDefaultName")}
      </div>
      <div ref={cardRef} className={`onboarding-card w-full max-w-lg bg-bg-base rounded-3xl shadow-2xl border ${themeStyles.borderTheme} relative min-h-[400px] z-30`} aria-busy={isFinishing}>
          {/* STEP 0: API Key */}
          {step === 0 && (
            <div
              key="step0"
              className="onboarding-step p-10 flex flex-col h-full justify-center"
            >
              <div className="text-center mb-8">
                <img src="/logo.png" alt="Logo" className="w-20 h-20 mx-auto mb-6 object-contain" />
                <h1 className={`text-3xl font-bold ${themeStyles.textBase} mb-2`}>连接你的 API</h1>
                <p className={`${themeStyles.textSecondary} text-sm`}>请输入你的 DeepSeek API Key 激活桌面客户端</p>
              </div>

              <div className="space-y-6">
                <div>
                  <label className={`block text-sm font-medium ${themeStyles.textBase} mb-2`}>API Key</label>
                  <div className="relative">
                    <div className="absolute inset-y-0 left-0 pl-3.5 flex items-center pointer-events-none">
                      <Lock className="h-3.5 w-3.5 text-text-secondary opacity-70" aria-hidden="true" />
                    </div>
                    <input
                      type="password"
                      value={apiKey}
                      onChange={(e) => setApiKey(e.target.value)}
                      placeholder="sk-..."
                      className={`block w-full pl-10 pr-4 py-3 border ${themeStyles.borderTheme} rounded-xl text-sm focus:ring-2 focus:ring-primary/20 focus:border-primary bg-transparent outline-none transition-all ${themeStyles.textBase}`}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") handleConnect();
                      }}
                    />
                  </div>
                </div>

                <button
                  onClick={handleConnect}
                  disabled={!apiKey.trim() || isConnecting}
                  className={`w-full flex justify-center items-center py-3 px-4 border border-transparent rounded-xl shadow-sm text-sm font-medium ${themeStyles.primaryText} ${themeStyles.primaryBg} focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-[#111827] disabled:opacity-50 disabled:cursor-not-allowed transition-all`}
                >
                  {isConnecting ? (
                    <>
                      <LoaderCircle className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden="true" />
                      连接中...
                    </>
                  ) : (
                    "确定连接"
                  )}
                </button>
              </div>
            </div>
          )}

          {/* STEP 1: Theme */}
          {step === 1 && (
            <div
              key="step1"
              className="onboarding-step p-10 flex flex-col h-full"
            >
              <div className="text-center mb-8">
                <div className="w-16 h-16 bg-sidebar-bg rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-sm border border-border-theme">
                  <Monitor className="h-6 w-6 text-text-base" aria-hidden="true" />
                </div>
                <h1 className={`text-2xl font-bold ${themeStyles.textBase} mb-2`}>选择系统主题</h1>
                <p className={`${themeStyles.textSecondary} text-sm`}>自定义界面的外观</p>
              </div>

              <div className="flex-1">
                <div className="flex space-x-3">
                  {[
                    { id: "light", icon: Sun, label: "浅色" },
                    { id: "dark", icon: Moon, label: "深色" },
                    { id: "system", icon: Monitor, label: "系统" },
                  ].map((tItem) => (
                    <button
                      key={tItem.id}
                      type="button"
                      onClick={(event) => {
                        const mode = tItem.id as ThemeMode;
                        const rect = event.currentTarget.getBoundingClientRect();
                        setTheme(mode);
                        switchTheme(mode, {
                          x: rect.left + rect.width / 2,
                          y: rect.top + rect.height / 2,
                        });
                      }}
                      className={`flex-1 flex flex-col items-center justify-center py-6 border rounded-xl text-sm transition-all ${
                        theme === tItem.id
                          ? `${themeStyles.selectedBorder} ${themeStyles.selectedBg} text-text-base shadow-sm`
                          : `${themeStyles.borderTheme} bg-transparent ${themeStyles.textSecondary} hover:bg-sidebar-bg`
                      }`}
                    >
                      <tItem.icon className="mb-2 h-[18px] w-[18px]" aria-hidden="true" />
                      <span className="font-medium">{tItem.label}</span>
                    </button>
                  ))}
                </div>
              </div>

              <div className="flex space-x-3 mt-8">
                <button
                  onClick={nextStep}
                  className={`flex-1 py-3 px-4 rounded-xl text-sm font-medium ${themeStyles.primaryText} ${themeStyles.primaryBg} transition-colors`}
                >
                  下一步
                </button>
              </div>
            </div>
          )}

          {/* STEP 2: Language */}
          {step === 2 && (
            <div
              key="step2"
              className="onboarding-step p-10 flex flex-col h-full"
            >
              <div className="text-center mb-8">
                <div className="w-16 h-16 bg-sidebar-bg rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-sm border border-border-theme">
                  <Globe className="h-6 w-6 text-text-base" aria-hidden="true" />
                </div>
                <h1 className={`text-2xl font-bold ${themeStyles.textBase} mb-2`}>选择语言</h1>
                <p className={`${themeStyles.textSecondary} text-sm`}>请选择你熟悉的系统语言</p>
              </div>

              <div className="flex-1">
                <div className="relative mt-2">
                  <button
                    type="button"
                    onClick={() => setIsLangDropdownOpen(!isLangDropdownOpen)}
                    className={`relative w-full pl-4 pr-10 py-3 text-left text-base border ${themeStyles.borderTheme} focus:outline-none focus:ring-2 focus:ring-primary focus:border-primary sm:text-sm rounded-xl bg-transparent ${themeStyles.textBase} transition-all`}
                  >
                    <span className="block truncate">
                      {SUPPORTED_LANGUAGES.find((l) => l.id === language)?.label}
                    </span>
                    <span className="pointer-events-none absolute inset-y-0 right-0 flex items-center pr-4 text-text-secondary opacity-70">
                      <ChevronDown className={`h-3.5 w-3.5 transition-transform duration-200 ${isLangDropdownOpen ? "rotate-180" : ""}`} aria-hidden="true" />
                    </span>
                  </button>

                    {isLangDropdownOpen && (
                      <>
                        <div 
                          className="fixed inset-0 z-40" 
                          onClick={() => setIsLangDropdownOpen(false)}
                        ></div>
                        <div
                          className={`popover-menu absolute z-50 mt-2 w-full bg-bg-base rounded-xl shadow-lg border ${themeStyles.borderTheme} overflow-hidden max-h-60 overflow-y-auto`}
                        >
                          <ul className="py-1">
                            {SUPPORTED_LANGUAGES.map((l) => (
                              <li
                                key={l.id}
                                onClick={() => {
                                  setLanguage(l.id);
                                  setIsLangDropdownOpen(false);
                                  i18n.changeLanguage(l.id);
                                }}
                                className={`cursor-pointer select-none relative py-2.5 pl-4 pr-9 hover:bg-sidebar-bg transition-colors ${
                                  language === l.id ? "bg-sidebar-bg text-text-base font-medium" : themeStyles.textBase
                                }`}
                              >
                                <span className="block truncate">{l.label}</span>
                                {language === l.id && (
                                  <span className="absolute inset-y-0 right-0 flex items-center pr-4 text-text-base">
                                    <Check className="h-3.5 w-3.5" aria-hidden="true" />
                                  </span>
                                )}
                              </li>
                            ))}
                          </ul>
                        </div>
                      </>
                    )}
                </div>
              </div>

              <div className="flex space-x-3 mt-8">
                <button
                  onClick={prevStep}
                  className={`px-6 py-3 rounded-xl text-sm font-medium border ${themeStyles.borderTheme} ${themeStyles.textSecondary} hover:bg-gray-50 transition-colors`}
                >
                  上一步
                </button>
                <button
                  onClick={nextStep}
                  className={`flex-1 py-3 px-4 rounded-xl text-sm font-medium ${themeStyles.primaryText} ${themeStyles.primaryBg} transition-colors`}
                >
                  下一步
                </button>
              </div>
            </div>
          )}

          {/* STEP 3: Sandbox */}
          {step === 3 && (
            <div
              key="step3"
              className="onboarding-step p-10 flex flex-col h-full"
            >
              <div className="text-center mb-8">
                <div className="w-16 h-16 bg-sidebar-bg rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-sm border border-border-theme">
                  <Shield className="h-6 w-6 text-text-base" aria-hidden="true" />
                </div>
                <h1 className={`text-2xl font-bold ${themeStyles.textBase} mb-2`}>沙箱权限</h1>
                <p className={`${themeStyles.textSecondary} text-sm`}>设置工具最多可以访问和修改哪里</p>
              </div>

              <div className="flex-1 space-y-3">
                {[
                  { id: "read_only", title: "只读模式", desc: "只能阅读当前项目文件，不能修改文件。", icon: Eye },
                  { id: "workspace_write", title: "项目内读写", desc: "允许读取和修改当前项目，推荐日常开发使用。", icon: FolderOpen },
                  { id: "full_access", title: "完全访问", desc: "允许读写项目外文件，适合明确需要跨目录操作时使用。", icon: LockOpen },
                ].map((m) => (
                  <button
                    key={m.id}
                    type="button"
                    onClick={() => setSandboxModeState(m.id as SandboxMode)}
                    className={`w-full flex items-start gap-3 p-4 border rounded-xl text-left transition-all ${
                      sandboxMode === m.id
                        ? `${themeStyles.selectedBorder} ${themeStyles.selectedBg} shadow-sm`
                        : `${themeStyles.borderTheme} bg-transparent hover:bg-sidebar-bg`
                    }`}
                  >
                    <div className={`mt-0.5 w-5 h-5 rounded-full border-2 flex items-center justify-center flex-shrink-0 ${sandboxMode === m.id ? "border-text-base" : "border-border-theme"}`}>
                      {sandboxMode === m.id && <div className="w-2.5 h-2.5 rounded-full bg-text-base" />}
                    </div>
                    <div className="min-w-0 flex-1">
                      <div className={`text-sm font-medium mb-1 ${sandboxMode === m.id ? "text-text-base" : themeStyles.textBase}`}>
                        {m.title}
                        {m.id === "workspace_write" && <span className="ml-2 text-[11px] text-text-secondary opacity-80">推荐</span>}
                      </div>
                      <div className={`text-xs leading-5 ${sandboxMode === m.id ? "text-text-base" : themeStyles.textSecondary}`}>
                        {m.desc}
                      </div>
                    </div>
                    <m.icon className={`mt-0.5 h-4 w-4 shrink-0 ${sandboxMode === m.id ? "text-text-base" : "text-text-secondary opacity-70"}`} aria-hidden="true" />
                  </button>
                ))}
              </div>

              <div className="flex space-x-3 mt-8">
                <button
                  onClick={prevStep}
                  className={`px-6 py-3 rounded-xl text-sm font-medium border ${themeStyles.borderTheme} ${themeStyles.textSecondary} hover:bg-sidebar-bg transition-colors`}
                >
                  上一步
                </button>
                <button
                  onClick={nextStep}
                  className={`flex-1 py-3 px-4 rounded-xl text-sm font-medium ${themeStyles.primaryText} ${themeStyles.primaryBg} transition-colors`}
                >
                  下一步
                </button>
              </div>
            </div>
          )}

          {/* STEP 4: Work Mode */}
          {step === 4 && (
            <div
              key="step4"
              className="onboarding-step p-10 flex flex-col h-full"
            >
              <div className="text-center mb-8">
                <div className="w-16 h-16 bg-sidebar-bg rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-sm border border-border-theme">
                  <SlidersHorizontal className="h-6 w-6 text-text-base" aria-hidden="true" />
                </div>
                <h1 className={`text-2xl font-bold ${themeStyles.textBase} mb-2`}>工作模式</h1>
                <p className={`${themeStyles.textSecondary} text-sm`}>选择 Codex 显示多少技术细节</p>
              </div>

              <div className="flex-1 flex space-x-3">
                {[
                  { id: "code", title: "适用于编程", desc: "更具技术性的回复和控制", icon: Terminal },
                  { id: "daily", title: "适用于日常工作", desc: "同样强大，技术细节更少", icon: MessagesSquare },
                ].map((m) => (
                  <Button
                    variant="ghost"
                    disabled={isFinishing}
                    key={m.id}
                    onClick={() => setWorkMode(m.id as "code" | "daily")}
                    className={`h-auto flex-1 flex flex-col items-start whitespace-normal p-4 border rounded-xl cursor-pointer transition-all ${
                      workMode === m.id
                        ? `${themeStyles.selectedBorder} ${themeStyles.selectedBg} shadow-sm`
                        : `${themeStyles.borderTheme} bg-transparent hover:bg-sidebar-bg`
                    }`}
                  >
                    <div className="flex items-center w-full mb-3">
                      <div className={`w-5 h-5 rounded-full border-2 flex items-center justify-center flex-shrink-0 ${workMode === m.id ? 'border-text-base' : 'border-border-theme'}`}>
                        {workMode === m.id && <div className="w-2.5 h-2.5 rounded-full bg-text-base" />}
                      </div>
                      <m.icon className={`ml-auto h-4 w-4 ${workMode === m.id ? 'text-text-base' : 'text-text-secondary opacity-70'}`} aria-hidden="true" />
                    </div>
                    <div className={`text-sm font-medium mb-1 ${workMode === m.id ? 'text-text-base' : themeStyles.textBase}`}>
                      {m.title}
                    </div>
                    <div className={`text-xs ${workMode === m.id ? 'text-text-base' : themeStyles.textSecondary}`}>
                      {m.desc}
                    </div>
                  </Button>
                ))}
              </div>

              <div className="flex space-x-3 mt-8">
                <Button
                  variant="outline"
                  disabled={isFinishing}
                  onClick={prevStep}
                  className={`h-auto px-6 py-3 rounded-xl text-sm font-medium border ${themeStyles.borderTheme} ${themeStyles.textSecondary} hover:bg-sidebar-bg transition-colors`}
                >
                  上一步
                </Button>
                <Button
                  onClick={handleFinish}
                  disabled={isFinishing}
                  className={`h-auto flex-1 flex justify-center items-center py-3 px-4 border border-transparent rounded-xl shadow-sm text-sm font-medium ${themeStyles.primaryText} ${themeStyles.primaryBg} transition-colors disabled:opacity-50 disabled:cursor-not-allowed`}
                >
                  {isFinishing ? (
                    <LoaderCircle className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden="true" />
                  ) : (
                    <Check className="mr-2 h-4 w-4" aria-hidden="true" />
                  )}
                  {isFinishing ? "保存中..." : "进入系统"}
                </Button>
              </div>
            </div>
          )}
      </div>
    </div>
  );
}
