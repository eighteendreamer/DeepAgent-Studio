import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, Eye, EyeOff, FolderOpen, Globe, LoaderCircle, Lock, LockOpen, MessagesSquare, Monitor, Moon, Shield, SlidersHorizontal, Sun, Terminal } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls, useReducedMotion } from "framer-motion";
import { getWelcomeName, initializeProject, isTauri, openExternalUrl, setSandboxMode, setWelcomeName as persistWelcomeName, type SandboxMode } from "../api";
import { message } from "./message";
import { useTranslation } from "react-i18next";
import { useTheme, type ThemeMode } from "../hooks/useTheme";
import { startOnboardingEntrance, type OnboardingEntrance } from "./onboardingEntrance";
import { ONBOARDING_EASE, OnboardingChoices, OnboardingPanel } from "./onboardingMotion";
import { Button } from "./shadcn/button";
import { Input } from "./shadcn/input";
import { Label } from "./shadcn/label";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "./shadcn/dropdown-menu";

const ConnectButton = motion.create(Button);
const STEPS = [
  { title: "连接你的 API", description: "", icon: Lock },
  { title: "选择系统主题", description: "自定义界面的外观", icon: Monitor },
  { title: "选择语言", description: "请选择你熟悉的系统语言", icon: Globe },
  { title: "沙箱权限", description: "设置工具最多可以访问和修改哪里", icon: Shield },
  { title: "工作模式", description: "选择适合你的工作方式", icon: SlidersHorizontal },
] as const;

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

export function OnboardingWizard({ onComplete }: { onComplete: () => void }) {
  const [step, setStep] = useState(0);
  const [direction, setDirection] = useState(1);
  const [moving, setMoving] = useState(false);
  const movingRef = useRef(false);
  const [panelHeight, setPanelHeight] = useState<number>();
  const [apiKey, setApiKey] = useState("");
  const [keyVisible, setKeyVisible] = useState(false);
  const [connectState, setConnectState] = useState<"idle" | "connecting" | "success" | "error">("idle");
  const [connectError, setConnectError] = useState("");
  const [isConnecting, setIsConnecting] = useState(false);
  const connectPendingRef = useRef(false);
  const mountedRef = useRef(true);
  const connectAnimation = useAnimationControls();
  const reducedMotion = useReducedMotion();
  const [isFinishing, setIsFinishing] = useState(false);
  const [finishError, setFinishError] = useState("");
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
  const inputRef = useRef<HTMLInputElement>(null);
  const finishButtonRef = useRef<HTMLButtonElement>(null);
  const { t, i18n } = useTranslation();
  const { activeIsDark, updateConfig, config } = useTheme();
  const [theme, setTheme] = useState<ThemeMode>(config.mode);
  const [language, setLanguage] = useState(() => SUPPORTED_LANGUAGES.some(item => item.id === i18n.language) ? i18n.language : "zh");
  const [isLangDropdownOpen, setIsLangDropdownOpen] = useState(false);
  const [workMode, setWorkMode] = useState<"code" | "daily">("code");
  const [sandboxMode, setSandboxModeState] = useState<SandboxMode>("workspace_write");
  const [welcomeName, setWelcomeName] = useState(() => localStorage.getItem("userName")?.trim() || "");

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      finishPendingRef.current = false;
      entranceRef.current?.cancel();
      connectAnimation.stop();
    };
  }, [connectAnimation]);

  useEffect(() => {
    if (!isFinishing && finishError) finishButtonRef.current?.focus();
  }, [finishError, isFinishing]);

  useEffect(() => {
    let active = true;
    const legacyName = localStorage.getItem("userName")?.trim() || "";
    getWelcomeName()
      .then(async storedName => {
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
      .catch(error => console.error("failed to load welcome name:", error));
    return () => { active = false; };
  }, []);

  const goToStep = (next: number) => {
    if (movingRef.current || finishPendingRef.current || next < 0 || next >= STEPS.length) return;
    movingRef.current = true;
    setMoving(true);
    setDirection(next > step ? 1 : -1);
    setIsLangDropdownOpen(false);
    setStep(next);
  };

  const handleConnect = async () => {
    const key = apiKey.trim();
    if (!key || connectPendingRef.current || movingRef.current) return;
    if (!isTauri()) {
      setConnectError("连接验证需要在桌面应用中进行，请运行桌面客户端。");
      return;
    }
    connectPendingRef.current = true;
    setIsConnecting(true);
    setConnectError("");
    setConnectState("connecting");
    const transition = { duration: reducedMotion ? 0 : 0.38, ease: ONBOARDING_EASE };
    try {
      const validation = initializeProject(key).then(
        () => ({ ok: true as const }),
        (error: unknown) => ({ ok: false as const, error }),
      );
      // Authentication and the visual contraction run together. Only the backend can advance the flow.
      const [result] = await Promise.all([validation, connectAnimation.start("collapsed", transition)]);
      if (!mountedRef.current) return;
      setConnectState(result.ok ? "success" : "error");
      await connectAnimation.start("expanded", transition);
      if (!mountedRef.current) return;
      if (result.ok) {
        goToStep(1);
      } else {
        console.error("initialize_project failed:", result.error);
        setConnectError("连接失败，请检查 API Key 或网络后重试。");
        inputRef.current?.focus();
      }
    } catch (error) {
      if (!mountedRef.current) return;
      console.error("onboarding connection failed:", error);
      setConnectState("error");
      setConnectError("连接过程失败，请重试。");
    } finally {
      connectPendingRef.current = false;
      if (mountedRef.current) {
        setIsConnecting(false);
        setConnectState(current => current === "success" ? "idle" : current);
      }
    }
  };

  const handleFinish = async () => {
    if (finishPendingRef.current || movingRef.current) return;
    finishPendingRef.current = true;
    setIsFinishing(true);
    setFinishError("");
    try {
      await setSandboxMode(sandboxMode);
      if (!finishPendingRef.current) return;
      // Keep existing settings keys and the backend permission boundary.
      localStorage.setItem("appLanguage", language);
      localStorage.setItem("workMode", workMode);
      localStorage.setItem("onboarding_complete", "true");
    } catch (error) {
      if (!finishPendingRef.current) return;
      console.error("onboarding settings save failed:", error);
      setFinishError("初始化设置保存失败，请重试。");
      finishPendingRef.current = false;
      setIsFinishing(false);
      return;
    }
    try {
      const overlay = overlayRef.current, card = cardRef.current, welcome = welcomeRef.current;
      const point = pointRef.current, leftDoor = leftDoorRef.current, rightDoor = rightDoorRef.current;
      const leftSeam = leftSeamRef.current, rightSeam = rightSeamRef.current;
      if (!overlay || !card || !welcome || !point || !leftDoor || !rightDoor || !leftSeam || !rightSeam) {
        throw new Error("onboarding entrance elements are unavailable");
      }
      entranceRef.current = startOnboardingEntrance({
        overlay, card, welcome, point, doors: [leftDoor, rightDoor], seams: [leftSeam, rightSeam],
      }, window.matchMedia("(prefers-reduced-motion: reduce)").matches);
      const completed = await entranceRef.current.finished;
      if (completed && finishPendingRef.current) onComplete();
    } catch (error) {
      if (!finishPendingRef.current) return;
      entranceRef.current?.cancel();
      entranceRef.current = null;
      console.error("onboarding entrance failed:", error);
      setFinishError("进入系统失败，请重试。");
      finishPendingRef.current = false;
      setIsFinishing(false);
    }
  };

  const current = STEPS[step];
  const transitionColor = activeIsDark ? "rgba(255,255,255,.72)" : "rgba(17,24,39,.58)";
  const transitionShadow = activeIsDark ? "0 0 12px rgba(255,255,255,.32)" : "0 0 12px rgba(17,24,39,.18)";
  const doorSurface = activeIsDark ? "rgba(255,255,255,.06)" : "rgba(17,24,39,.045)";

  return (
    <div ref={overlayRef} className="onboarding-overlay absolute top-8 inset-x-0 bottom-0 z-50 flex items-center justify-center overflow-hidden" role="dialog" aria-modal="true" aria-label="初始化向导">
      <div className="absolute inset-0 z-10 pointer-events-none" aria-hidden="true">
        <div ref={leftDoorRef} className="onboarding-door absolute inset-y-0 left-0 w-1/2 bg-bg-base">
          <div className="absolute inset-0" style={{ backgroundColor: doorSurface }} />
          <div ref={leftSeamRef} className="onboarding-door-line absolute right-0 inset-y-0 w-px" style={{ backgroundColor: transitionColor, boxShadow: transitionShadow }} />
        </div>
        <div ref={rightDoorRef} className="onboarding-door absolute inset-y-0 right-0 w-1/2 bg-bg-base">
          <div className="absolute inset-0" style={{ backgroundColor: doorSurface }} />
          <div ref={rightSeamRef} className="onboarding-door-line absolute left-0 inset-y-0 w-px" style={{ backgroundColor: transitionColor, boxShadow: transitionShadow }} />
        </div>
      </div>
      <div ref={pointRef} aria-hidden="true" className="onboarding-transition-dot absolute left-1/2 top-1/2 z-20 h-px w-px rounded-full" style={{ backgroundColor: transitionColor, boxShadow: transitionShadow }} />
      <div ref={welcomeRef} aria-hidden="true" className="onboarding-welcome-text absolute z-50 text-3xl font-bold tracking-wider text-text-base pointer-events-none">
        {t("settings.personalize.greetingPrefix")}{welcomeName || t("settings.personalize.greetingDefaultName")}
      </div>
      <div ref={cardRef} className="onboarding-card relative z-30 w-full max-w-lg rounded-3xl border border-border-theme bg-bg-base text-text-base shadow-2xl" aria-busy={isFinishing || moving}>
        <motion.div initial={false} animate={{ height: panelHeight ?? "auto" }} transition={{ duration: reducedMotion ? 0 : 0.28, ease: ONBOARDING_EASE }} className="relative" style={{ overflow: moving ? "hidden" : "visible" }}>
          <AnimatePresence initial={false} mode="popLayout" custom={direction}>
            <OnboardingPanel key={step} direction={direction} disabled={moving || isFinishing} onHeight={setPanelHeight} onEntered={() => { movingRef.current = false; setMoving(false); }}>
              <header className="onboarding-panel-header text-center">
                <div className={`onboarding-emblem ${step === 0 ? "onboarding-brand" : ""}`}>
                  {step === 0 ? <img src="/logo.png" alt="DeepAgent Studio" /> : <current.icon aria-hidden="true" />}
                </div>
                <h1 tabIndex={-1} className={`mb-2 font-bold outline-none ${step === 0 ? "text-3xl" : "text-2xl"}`}>{current.title}</h1>
                {step === 0 ? (
                  <p className="text-sm text-text-secondary">
                    请输入你的{" "}
                    <a href="https://platform.deepseek.com/api_keys" target="_blank" rel="noopener noreferrer" className="text-primary underline underline-offset-2" onClick={event => {
                      if (!isTauri()) return;
                      event.preventDefault();
                      void openExternalUrl(event.currentTarget.href).catch(error => message.error(`打开密钥管理页面失败：${String(error)}`));
                    }}>DeepSeek API Key</a>{" "}激活桌面客户端
                  </p>
                ) : <p className="text-sm text-text-secondary">{current.description}</p>}
              </header>
              {step === 0 && (
                <div>
                  <Label htmlFor="onboarding-api-key" className="mb-2 block text-sm text-text-base">API Key</Label>
                  <div className={`onboarding-key-field ${connectError ? "onboarding-key-invalid" : ""}`}>
                    <Lock className="h-[15px] w-[15px] shrink-0 text-text-secondary" aria-hidden="true" />
                    <Input ref={inputRef} id="onboarding-api-key" type={keyVisible ? "text" : "password"} value={apiKey} readOnly={isConnecting} autoComplete="off" spellCheck={false} placeholder="sk-..." aria-invalid={!!connectError} aria-describedby={connectError ? "onboarding-connect-error" : undefined} className="h-full min-w-0 flex-1 rounded-none border-0 bg-transparent p-0 text-sm shadow-none focus:ring-0"
                      onChange={event => { setApiKey(event.target.value); setConnectError(""); setConnectState("idle"); }}
                      onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); void handleConnect(); } }} />
                    <Button variant="ghost" size="icon" disabled={isConnecting} className="h-6 w-6 rounded text-text-secondary hover:bg-ui-tint" aria-label={keyVisible ? "隐藏密钥" : "显示密钥"} onClick={() => setKeyVisible(value => !value)}>
                      {keyVisible ? <EyeOff className="h-[15px] w-[15px]" /> : <Eye className="h-[15px] w-[15px]" />}
                    </Button>
                  </div>
                  <div id="onboarding-connect-error" className={`onboarding-inline-error ${connectError ? "onboarding-error-visible" : ""}`} role="alert"><div>{connectError}</div></div>
                </div>
              )}
              {step === 1 && <OnboardingChoices<ThemeMode> label="系统主题" value={theme} layout="theme" onChange={mode => { setTheme(mode); updateConfig({ mode }); }} options={[{ id: "light", title: "浅色", icon: Sun }, { id: "dark", title: "深色", icon: Moon }, { id: "system", title: "系统", icon: Monitor }]} />}
              {step === 2 && (
                <DropdownMenu open={isLangDropdownOpen} onOpenChange={setIsLangDropdownOpen} modal={false}>
                  <DropdownMenuTrigger asChild>
                    <Button variant="outline" className="onboarding-language-trigger h-[46px] w-full justify-between rounded-xl bg-transparent px-4 text-sm hover:bg-ui-tint" aria-label="系统语言">
                      <span>{SUPPORTED_LANGUAGES.find(item => item.id === language)?.label}</span><ChevronDown aria-hidden="true" className={`h-4 w-4 transition-transform ${isLangDropdownOpen ? "rotate-180" : ""}`} />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="start" sideOffset={8} className="onboarding-language-menu max-h-60 w-[var(--radix-dropdown-menu-trigger-width)] overflow-y-auto rounded-xl border border-border-theme bg-bg-base p-1">
                    {SUPPORTED_LANGUAGES.map(item => <DropdownMenuItem key={item.id} role="menuitemradio" aria-checked={language === item.id} className="justify-between px-3 py-2.5 text-[13px] focus:bg-ui-tint" onSelect={() => {
                      void i18n.changeLanguage(item.id).then(() => { if (mountedRef.current) setLanguage(item.id); }).catch(error => { console.error("onboarding language change failed:", error); message.error("语言切换失败，请重试"); });
                    }}><span>{item.label}</span>{language === item.id && <Check className="h-3.5 w-3.5" aria-hidden="true" />}</DropdownMenuItem>)}
                  </DropdownMenuContent>
                </DropdownMenu>
              )}
              {step === 3 && <OnboardingChoices<SandboxMode> label="沙箱权限" value={sandboxMode} onChange={setSandboxModeState} layout="sandbox" options={[
                { id: "read_only", title: "只读模式", description: "只能阅读当前项目文件，不能修改文件。", icon: Eye },
                { id: "workspace_write", title: "项目内读写", description: "允许读取和修改当前项目，推荐日常开发使用。", icon: FolderOpen, recommended: true },
                { id: "full_access", title: "完全访问", description: "允许读写项目外文件，适合跨目录操作时使用。", icon: LockOpen },
              ]} />}
              {step === 4 && <OnboardingChoices<"code" | "daily"> label="工作模式" value={workMode} onChange={setWorkMode} layout="work" options={[
                { id: "code", title: "适用于编程", description: "更具技术性的回复和控制", icon: Terminal },
                { id: "daily", title: "适用于日常工作", description: "同样强大，技术细节更少", icon: MessagesSquare },
              ]} />}
              {step === 4 && <div className={`onboarding-inline-error ${finishError ? "onboarding-error-visible" : ""}`} role="alert"><div>{finishError}</div></div>}
              <footer className={`onboarding-actions ${step === 0 ? "onboarding-actions-connect" : ""}`}>
                {step > 0 && <Button variant="outline" className="onboarding-back" onClick={() => goToStep(step - 1)}>上一步</Button>}
                {step === 0 ? (
                  <ConnectButton initial="expanded" animate={connectAnimation} variants={{ expanded: { width: "100%", borderRadius: "12px" }, collapsed: { width: "46px", borderRadius: "23px" } }} onClick={handleConnect} disabled={!apiKey.trim() || isConnecting} aria-busy={isConnecting} aria-label={{ idle: "确定连接", connecting: "正在连接", success: "连接成功", error: "重试连接" }[connectState]} data-connect-state={connectState} className="onboarding-primary w-full min-w-0 overflow-hidden px-0 disabled:opacity-50 data-[connect-state=connecting]:disabled:opacity-100 data-[connect-state=success]:disabled:opacity-100">
                    <motion.span key={connectState} initial={reducedMotion ? false : { opacity: 0 }} animate={{ opacity: 1 }} transition={{ duration: reducedMotion ? 0 : 0.18 }} className="inline-flex items-center justify-center gap-2 whitespace-nowrap" aria-hidden="true">
                      {connectState === "connecting" ? <LoaderCircle className="h-4 w-4 motion-safe:animate-spin" /> : connectState === "success" ? <><Check className="h-4 w-4" />连接成功</> : connectState === "error" ? "重试连接" : "确定连接"}
                    </motion.span>
                  </ConnectButton>
                ) : step === 4 ? (
                  <Button ref={finishButtonRef} onClick={handleFinish} disabled={isFinishing} className="onboarding-primary flex-1" aria-busy={isFinishing}>{isFinishing && <LoaderCircle className="h-4 w-4 motion-safe:animate-spin" aria-hidden="true" />}{isFinishing ? "保存中…" : "进入系统"}</Button>
                ) : <Button className="onboarding-primary flex-1" onClick={() => goToStep(step + 1)}>下一步</Button>}
              </footer>
            </OnboardingPanel>
          </AnimatePresence>
        </motion.div>
      </div>
    </div>
  );
}
