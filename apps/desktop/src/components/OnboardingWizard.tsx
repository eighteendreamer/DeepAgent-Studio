import { useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { motion, AnimatePresence } from "framer-motion";
import { initializeProject, isTauri } from "../api";
import { message } from "./message";

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
  const [step, setStep] = useState<0 | 1 | 2 | 3>(0);
  const [apiKey, setApiKey] = useState("");
  const [isConnecting, setIsConnecting] = useState(false);
  
  const [theme, setTheme] = useState<"light" | "dark" | "system">("system");
  const [language, setLanguage] = useState("zh");
  const [isLangDropdownOpen, setIsLangDropdownOpen] = useState(false);
  const [workMode, setWorkMode] = useState<"code" | "daily">("code");

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

  const handleFinish = () => {
    // Save settings locally
    localStorage.setItem("theme", theme);
    localStorage.setItem("language", language);
    localStorage.setItem("workMode", workMode);
    localStorage.setItem("onboarding_complete", "true");
    onComplete();
  };

  const nextStep = () => setStep((s) => Math.min(s + 1, 3) as any);
  const prevStep = () => setStep((s) => Math.max(s - 1, 1) as any);

  // Custom theme classes based on user request
  const themeStyles = {
    primaryBg: "bg-[#111827] hover:bg-[#000000]",
    primaryText: "text-white",
    textBase: "text-[#1F2937]",
    textSecondary: "text-[#6B7280]",
    borderTheme: "border-[#E5E7EB]",
    selectedBorder: "border-[#111827]",
    selectedBg: "bg-gray-50",
  };

  return (
    <div className="absolute top-10 inset-x-0 bottom-0 z-50 flex items-center justify-center bg-gray-50/95 backdrop-blur-md overflow-hidden">
      <div className={`w-full max-w-lg bg-white rounded-3xl shadow-2xl border ${themeStyles.borderTheme} relative min-h-[400px]`}>
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
                      <FontAwesomeIcon icon={["fas", "lock"]} className="text-gray-400 text-sm" />
                    </div>
                    <input
                      type="password"
                      value={apiKey}
                      onChange={(e) => setApiKey(e.target.value)}
                      placeholder="sk-..."
                      className={`block w-full pl-10 pr-4 py-3 border ${themeStyles.borderTheme} rounded-xl text-sm focus:ring-2 focus:ring-[#111827]/20 focus:border-[#111827] bg-gray-50/50 outline-none transition-all ${themeStyles.textBase}`}
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
                <div className="w-16 h-16 bg-gray-100 rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-sm border border-gray-200">
                  <FontAwesomeIcon icon={["fas", "desktop"]} className="text-[#111827] text-2xl" />
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
                  ].map((t) => (
                    <button
                      key={t.id}
                      onClick={() => setTheme(t.id as any)}
                      className={`flex-1 flex flex-col items-center justify-center py-6 border rounded-xl text-sm transition-all ${
                        theme === t.id
                          ? `${themeStyles.selectedBorder} ${themeStyles.selectedBg} text-[#111827] shadow-sm`
                          : `${themeStyles.borderTheme} bg-white ${themeStyles.textSecondary} hover:bg-gray-50`
                      }`}
                    >
                      <FontAwesomeIcon icon={t.icon as any} className="mb-2 text-lg" />
                      <span className="font-medium">{t.label}</span>
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
                <div className="w-16 h-16 bg-gray-100 rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-sm border border-gray-200">
                  <FontAwesomeIcon icon={["fas", "globe"]} className="text-[#111827] text-2xl" />
                </div>
                <h1 className={`text-2xl font-bold ${themeStyles.textBase} mb-2`}>选择语言</h1>
                <p className={`${themeStyles.textSecondary} text-sm`}>请选择你熟悉的系统语言</p>
              </div>

              <div className="flex-1">
                <div className="relative mt-2">
                  <button
                    type="button"
                    onClick={() => setIsLangDropdownOpen(!isLangDropdownOpen)}
                    className={`relative w-full pl-4 pr-10 py-3 text-left text-base border ${themeStyles.borderTheme} focus:outline-none focus:ring-2 focus:ring-[#111827] focus:border-[#111827] sm:text-sm rounded-xl bg-gray-50/50 ${themeStyles.textBase} transition-all`}
                  >
                    <span className="block truncate">
                      {SUPPORTED_LANGUAGES.find((l) => l.id === language)?.label}
                    </span>
                    <span className="pointer-events-none absolute inset-y-0 right-0 flex items-center pr-4 text-gray-500">
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
                          className={`absolute z-50 mt-2 w-full bg-white rounded-xl shadow-lg border ${themeStyles.borderTheme} overflow-hidden max-h-60 overflow-y-auto`}
                        >
                          <ul className="py-1">
                            {SUPPORTED_LANGUAGES.map((l) => (
                              <li
                                key={l.id}
                                onClick={() => {
                                  setLanguage(l.id);
                                  setIsLangDropdownOpen(false);
                                }}
                                className={`cursor-pointer select-none relative py-2.5 pl-4 pr-9 hover:bg-gray-100 transition-colors ${
                                  language === l.id ? "bg-gray-50 text-[#111827] font-medium" : themeStyles.textBase
                                }`}
                              >
                                <span className="block truncate">{l.label}</span>
                                {language === l.id && (
                                  <span className="absolute inset-y-0 right-0 flex items-center pr-4 text-[#111827]">
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

          {/* STEP 3: Work Mode */}
          {step === 3 && (
            <motion.div
              key="step3"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              className="p-10 flex flex-col h-full"
            >
              <div className="text-center mb-8">
                <div className="w-16 h-16 bg-gray-100 rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-sm border border-gray-200">
                  <FontAwesomeIcon icon={["fas", "sliders"]} className="text-[#111827] text-2xl" />
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
                        : `${themeStyles.borderTheme} bg-white hover:bg-gray-50`
                    }`}
                  >
                    <div className="flex items-center w-full mb-3">
                      <div className={`w-5 h-5 rounded-full border-2 flex items-center justify-center flex-shrink-0 ${workMode === m.id ? 'border-[#111827]' : 'border-gray-300'}`}>
                        {workMode === m.id && <div className="w-2.5 h-2.5 rounded-full bg-[#111827]" />}
                      </div>
                      <FontAwesomeIcon icon={["fas", m.icon as any]} className={`ml-auto ${workMode === m.id ? 'text-[#111827]' : 'text-gray-400'}`} />
                    </div>
                    <div className={`text-sm font-medium mb-1 ${workMode === m.id ? 'text-[#111827]' : themeStyles.textBase}`}>
                      {m.title}
                    </div>
                    <div className={`text-xs ${workMode === m.id ? 'text-gray-700' : themeStyles.textSecondary}`}>
                      {m.desc}
                    </div>
                  </div>
                ))}
              </div>

              <div className="flex space-x-3 mt-8">
                <button
                  onClick={prevStep}
                  className={`px-6 py-3 rounded-xl text-sm font-medium border ${themeStyles.borderTheme} ${themeStyles.textSecondary} hover:bg-gray-50 transition-colors`}
                >
                  上一步
                </button>
                <button
                  onClick={handleFinish}
                  className={`flex-1 flex justify-center items-center py-3 px-4 border border-transparent rounded-xl shadow-sm text-sm font-medium ${themeStyles.primaryText} ${themeStyles.primaryBg} transition-colors`}
                >
                  <FontAwesomeIcon icon={["fas", "check"]} className="mr-2" />
                  进入系统
                </button>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}
