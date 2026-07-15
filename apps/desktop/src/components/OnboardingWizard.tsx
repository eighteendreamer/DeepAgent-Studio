import { useEffect, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { motion, AnimatePresence } from "framer-motion";
import { getWelcomeName, initializeProject, isTauri, setSandboxMode, setWelcomeName as persistWelcomeName, type SandboxMode } from "../api";
import { message } from "./message";
import { useTranslation } from "react-i18next";
import { useTheme } from "../hooks/useTheme";
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
  
  const { t, i18n } = useTranslation();
  const { updateConfig, activeIsDark } = useTheme();
  
  const [theme, setTheme] = useState<"light" | "dark" | "system">("system");
  const [language, setLanguage] = useState("zh");
  const [isLangDropdownOpen, setIsLangDropdownOpen] = useState(false);
  const [workMode, setWorkMode] = useState<"code" | "daily">("code");
  const [sandboxMode, setSandboxModeState] = useState<SandboxMode>("workspace_write");
  const [animStage, setAnimStage] = useState<'idle' | 'contentBlur' | 'welcomeText' | 'welcomeOut' | 'lineAppear' | 'doorsOpen'>('idle');
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
      // step 0. Only a valid key (stored in the OS keychain) advances.
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
    if (isFinishing) return;
    setIsFinishing(true);

    try {
      await setSandboxMode(sandboxMode);
    } catch (e) {
      message.error("沙箱设置保存失败，请重试");
      console.error("set_sandbox_mode failed:", e);
      setIsFinishing(false);
      return;
    }

    // Save settings locally with the correct keys used by the rest of the app
    
    // Theme uses codex-theme-config object
    try {
      const stored = localStorage.getItem("codex-theme-config");
      const config = stored ? JSON.parse(stored) : { mode: "system" };
      config.mode = theme;
      localStorage.setItem("codex-theme-config", JSON.stringify(config));
    } catch (e) {
      localStorage.setItem("codex-theme-config", JSON.stringify({ mode: theme }));
    }
    
    // Language uses appLanguage
    localStorage.setItem("appLanguage", language);
    
    // Work mode uses workMode
    localStorage.setItem("workMode", workMode);
    
    localStorage.setItem("onboarding_complete", "true");
    
    // Start the cinematic entrance animation
    setAnimStage('contentBlur');
    setTimeout(() => setAnimStage('welcomeText'), 600);
    setTimeout(() => setAnimStage('welcomeOut'), 2400); // Text starts exiting
    setTimeout(() => setAnimStage('lineAppear'), 3400); // Triggers exactly as text fades out
    setTimeout(() => setAnimStage('doorsOpen'), 4600); // Line grows for 1.2s, then doors open
    setTimeout(() => {
      onComplete();
      // We no longer reload here to preserve the smooth transition.
      // i18n and theme are already updated via state, so it should be fine.
    }, 6200); // wait for sliding door
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
    <div className="absolute top-10 inset-x-0 bottom-0 z-50 flex items-center justify-center overflow-hidden">
      {/* Keep a solid transition surface behind the animation. It fades only when
          the doors open, so the blurred card never exposes the app underneath. */}
      <motion.div
        aria-hidden="true"
        initial={{ opacity: 1 }}
        animate={{ opacity: animStage === 'doorsOpen' ? 0 : 1 }}
        transition={{ duration: 1.5, ease: [0.76, 0, 0.24, 1] }}
        className="absolute inset-0 z-0 bg-bg-base"
      />

      {/* Sliding Doors Backdrop */}
      <div className="absolute inset-0 z-10 pointer-events-none">
        <motion.div
            initial={{ width: '50%' }}
            animate={{ width: animStage === 'doorsOpen' ? '0%' : '50%' }}
            transition={{ duration: 1.5, ease: [0.76, 0, 0.24, 1] }}
            className="absolute top-0 bottom-0 left-0 backdrop-blur-md overflow-visible"
            style={{
              backgroundColor: doorSurface,
            }}
          >
            <div className="absolute right-0 top-0 bottom-0 flex items-center">
              <motion.div 
                initial={{ height: '0%', opacity: 0 }} 
                animate={{ 
                  height: (animStage === 'lineAppear' || animStage === 'doorsOpen') ? '100%' : '0%',
                  opacity: (animStage === 'lineAppear' || animStage === 'doorsOpen') ? 1 : 0 
                }}
                transition={{ duration: 1.2, ease: "easeInOut" }}
                className="w-[1px]"
                style={{ backgroundColor: transitionColor, boxShadow: transitionShadow }}
              />
            </div>
          </motion.div>
          <motion.div
            initial={{ width: '50%' }}
            animate={{ width: animStage === 'doorsOpen' ? '0%' : '50%' }}
            transition={{ duration: 1.5, ease: [0.76, 0, 0.24, 1] }}
            className="absolute top-0 bottom-0 right-0 backdrop-blur-md overflow-visible"
            style={{
              backgroundColor: doorSurface,
            }}
          >
            <div className="absolute left-0 top-0 bottom-0 flex items-center">
              <motion.div 
                initial={{ height: '0%', opacity: 0 }} 
                animate={{ 
                  height: (animStage === 'lineAppear' || animStage === 'doorsOpen') ? '100%' : '0%',
                  opacity: (animStage === 'lineAppear' || animStage === 'doorsOpen') ? 1 : 0 
                }}
                transition={{ duration: 1.2, ease: "easeInOut" }}
                className="w-[1px]"
                style={{ backgroundColor: transitionColor, boxShadow: transitionShadow }}
              />
            </div>
          </motion.div>
        </div>

        {/* The dot bridges the text exit and the vertical line without a blank frame. */}
        <motion.div
          initial={{ opacity: 0, scale: 0.4 }}
          animate={{
            opacity: (animStage === 'welcomeOut' || animStage === 'lineAppear' || animStage === 'doorsOpen') ? 1 : 0,
            scale: (animStage === 'welcomeOut' || animStage === 'lineAppear' || animStage === 'doorsOpen') ? 1 : 0.4,
          }}
          transition={{ duration: 0.35, ease: "easeOut" }}
          className="absolute left-1/2 top-1/2 z-20 h-[1px] w-[1px] -translate-x-1/2 -translate-y-1/2 rounded-full"
          style={{ backgroundColor: transitionColor, boxShadow: transitionShadow }}
        />

      {/* Welcome Text */}
      <AnimatePresence>
        {animStage === 'welcomeText' && (
          <motion.div
            initial={{ filter: 'blur(10px)', opacity: 0, scale: 0.95 }}
            animate={{ filter: 'blur(0px)', opacity: 1, scale: 1 }}
            exit={{ filter: 'blur(10px)', opacity: 0, scale: 1.05 }}
            transition={{ duration: 1.2 }}
            className={`absolute z-50 text-3xl font-bold tracking-wider ${themeStyles.textBase} pointer-events-none drop-shadow-lg`}
          >
            {t("settings.personalize.greetingPrefix")}{welcomeName || t("settings.personalize.greetingDefaultName")}
          </motion.div>
        )}
      </AnimatePresence>

      <motion.div 
        animate={animStage !== 'idle' ? { filter: 'blur(15px)', opacity: 0, scale: 0.9 } : { opacity: 1, scale: 1 }}
        transition={{ duration: 0.6 }}
        className={`w-full max-w-lg bg-bg-base rounded-3xl shadow-2xl border ${themeStyles.borderTheme} relative min-h-[400px] z-30`}
      >
        <AnimatePresence mode="wait">
          {/* STEP 0: API Key */}
          {step === 0 && (
            <motion.div
              key="step0"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              className="p-10 flex flex-col h-full justify-center"
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
                      <FontAwesomeIcon icon={["fas", "lock"]} className="text-text-secondary opacity-70 text-sm" />
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
                      <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin mr-2" />
                      连接中...
                    </>
                  ) : (
                    "确定连接"
                  )}
                </button>
              </div>
            </motion.div>
          )}

          {/* STEP 1: Theme */}
          {step === 1 && (
            <motion.div
              key="step1"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              className="p-10 flex flex-col h-full"
            >
              <div className="text-center mb-8">
                <div className="w-16 h-16 bg-sidebar-bg rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-sm border border-border-theme">
                  <FontAwesomeIcon icon={["fas", "desktop"]} className="text-text-base text-2xl" />
                </div>
                <h1 className={`text-2xl font-bold ${themeStyles.textBase} mb-2`}>选择系统主题</h1>
                <p className={`${themeStyles.textSecondary} text-sm`}>自定义界面的外观</p>
              </div>

              <div className="flex-1">
                <div className="flex space-x-3">
                  {[
                    { id: "light", icon: ["far", "sun"], label: "浅色" },
                    { id: "dark", icon: ["fas", "moon"], label: "深色" },
                    { id: "system", icon: ["fas", "desktop"], label: "系统" },
                  ].map((tItem) => (
                    <button
                      key={tItem.id}
                      onClick={() => {
                        setTheme(tItem.id as any);
                        updateConfig({ mode: tItem.id as any });
                      }}
                      className={`flex-1 flex flex-col items-center justify-center py-6 border rounded-xl text-sm transition-all ${
                        theme === tItem.id
                          ? `${themeStyles.selectedBorder} ${themeStyles.selectedBg} text-text-base shadow-sm`
                          : `${themeStyles.borderTheme} bg-transparent ${themeStyles.textSecondary} hover:bg-sidebar-bg`
                      }`}
                    >
                      <FontAwesomeIcon icon={tItem.icon as any} className="mb-2 text-lg" />
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
            </motion.div>
          )}

          {/* STEP 2: Language */}
          {step === 2 && (
            <motion.div
              key="step2"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              className="p-10 flex flex-col h-full"
            >
              <div className="text-center mb-8">
                <div className="w-16 h-16 bg-sidebar-bg rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-sm border border-border-theme">
                  <FontAwesomeIcon icon={["fas", "globe"]} className="text-text-base text-2xl" />
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
                      <FontAwesomeIcon icon={["fas", "chevron-down"]} className={`text-sm transition-transform duration-200 ${isLangDropdownOpen ? "rotate-180" : ""}`} />
                    </span>
                  </button>

                  <AnimatePresence>
                    {isLangDropdownOpen && (
                      <>
                        <div 
                          className="fixed inset-0 z-40" 
                          onClick={() => setIsLangDropdownOpen(false)}
                        ></div>
                        <motion.div
                          initial={{ opacity: 0, y: -10 }}
                          animate={{ opacity: 1, y: 0 }}
                          exit={{ opacity: 0, y: -10 }}
                          transition={{ duration: 0.15 }}
                          className={`absolute z-50 mt-2 w-full bg-bg-base rounded-xl shadow-lg border ${themeStyles.borderTheme} overflow-hidden max-h-60 overflow-y-auto`}
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
                                    <FontAwesomeIcon icon={["fas", "check"]} className="text-sm" />
                                  </span>
                                )}
                              </li>
                            ))}
                          </ul>
                        </motion.div>
                      </>
                    )}
                  </AnimatePresence>
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
            </motion.div>
          )}

          {/* STEP 3: Sandbox */}
          {step === 3 && (
            <motion.div
              key="step3"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              className="p-10 flex flex-col h-full"
            >
              <div className="text-center mb-8">
                <div className="w-16 h-16 bg-sidebar-bg rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-sm border border-border-theme">
                  <FontAwesomeIcon icon={["fas", "shield-halved"]} className="text-text-base text-2xl" />
                </div>
                <h1 className={`text-2xl font-bold ${themeStyles.textBase} mb-2`}>沙箱权限</h1>
                <p className={`${themeStyles.textSecondary} text-sm`}>设置工具最多可以访问和修改哪里</p>
              </div>

              <div className="flex-1 space-y-3">
                {[
                  { id: "read_only", title: "只读模式", desc: "只能阅读当前项目文件，不能修改文件。", icon: "eye" },
                  { id: "workspace_write", title: "项目内读写", desc: "允许读取和修改当前项目，推荐日常开发使用。", icon: "folder-open" },
                  { id: "full_access", title: "完全访问", desc: "允许读写项目外文件，适合明确需要跨目录操作时使用。", icon: "unlock" },
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
                    <FontAwesomeIcon icon={["fas", m.icon as any]} className={`mt-0.5 ${sandboxMode === m.id ? "text-text-base" : "text-text-secondary opacity-70"}`} />
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
            </motion.div>
          )}

          {/* STEP 4: Work Mode */}
          {step === 4 && (
            <motion.div
              key="step4"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              className="p-10 flex flex-col h-full"
            >
              <div className="text-center mb-8">
                <div className="w-16 h-16 bg-sidebar-bg rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-sm border border-border-theme">
                  <FontAwesomeIcon icon={["fas", "sliders"]} className="text-text-base text-2xl" />
                </div>
                <h1 className={`text-2xl font-bold ${themeStyles.textBase} mb-2`}>工作模式</h1>
                <p className={`${themeStyles.textSecondary} text-sm`}>选择 Codex 显示多少技术细节</p>
              </div>

              <div className="flex-1 flex space-x-3">
                {[
                  { id: "code", title: "适用于编程", desc: "更具技术性的回复和控制", icon: "terminal" },
                  { id: "daily", title: "适用于日常工作", desc: "同样强大，技术细节更少", icon: "comments" },
                ].map((m) => (
                  <div
                    key={m.id}
                    onClick={() => setWorkMode(m.id as any)}
                    className={`flex-1 flex flex-col items-start p-4 border rounded-xl cursor-pointer transition-all ${
                      workMode === m.id
                        ? `${themeStyles.selectedBorder} ${themeStyles.selectedBg} shadow-sm`
                        : `${themeStyles.borderTheme} bg-transparent hover:bg-sidebar-bg`
                    }`}
                  >
                    <div className="flex items-center w-full mb-3">
                      <div className={`w-5 h-5 rounded-full border-2 flex items-center justify-center flex-shrink-0 ${workMode === m.id ? 'border-text-base' : 'border-border-theme'}`}>
                        {workMode === m.id && <div className="w-2.5 h-2.5 rounded-full bg-text-base" />}
                      </div>
                      <FontAwesomeIcon icon={["fas", m.icon as any]} className={`ml-auto ${workMode === m.id ? 'text-text-base' : 'text-text-secondary opacity-70'}`} />
                    </div>
                    <div className={`text-sm font-medium mb-1 ${workMode === m.id ? 'text-text-base' : themeStyles.textBase}`}>
                      {m.title}
                    </div>
                    <div className={`text-xs ${workMode === m.id ? 'text-text-base' : themeStyles.textSecondary}`}>
                      {m.desc}
                    </div>
                  </div>
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
                  onClick={handleFinish}
                  disabled={isFinishing}
                  className={`flex-1 flex justify-center items-center py-3 px-4 border border-transparent rounded-xl shadow-sm text-sm font-medium ${themeStyles.primaryText} ${themeStyles.primaryBg} transition-colors disabled:opacity-50 disabled:cursor-not-allowed`}
                >
                  <FontAwesomeIcon icon={["fas", isFinishing ? "circle-notch" : "check"]} className={`mr-2 ${isFinishing ? "animate-spin" : ""}`} />
                  {isFinishing ? "保存中..." : "进入系统"}
                </button>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </motion.div>
    </div>
  );
}
