import { useState, useRef, useEffect, useCallback } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { SETTINGS_CHANGED_EVENT, getSettings, setChatModel, setThinkingDepth, getApprovalPolicy, setApprovalPolicy, getCommands } from "../api";
import { message } from "./message";
import type { Command } from "../types";

type SlashChoice = Command & { insertText?: string };

/** Map composer dropdown option id ↔ backend approval-policy label. */
const OPTION_TO_POLICY: Record<string, string> = {
  default: "always_ask",
  auto: "auto_review",
  full: "full_access",
};
const POLICY_TO_OPTION: Record<string, string> = {
  always_ask: "default",
  auto_review: "auto",
  full_access: "full",
};

/** Split a model id into a two-tier {name, version} label for display.
 * e.g. "deepseek-v4-flash" -> {name:"deepseek", version:"v4-flash"}. Ids without a
 * hyphen render as a single name. */
function labelFor(id: string): { name: string; version: string } {
  const dash = id.indexOf("-");
  if (dash <= 0) return { name: id, version: "" };
  return { name: id.slice(0, dash), version: id.slice(dash + 1) };
}

interface Props {
  value: string;
  onChange: (v: string) => void;
  onSubmit: () => void;
  placeholder?: string;
  /** True while a run is streaming: disables submit and shows a busy button. */
  busy?: boolean;
  /** Stop the in-flight run (turns the busy button into a stop button). */
  onStop?: () => void;
  /** True when the current session is in read-only Plan mode. */
  planMode?: boolean;
  /** Optional footer content rendered seamlessly at the bottom of the composer. */
  footer?: React.ReactNode;
}

export function Composer({ value, onChange, onSubmit, placeholder, busy = false, onStop, planMode = false, footer }: Props) {
  const { t } = useTranslation();
  const [isModelDropdownOpen, setIsModelDropdownOpen] = useState(false);
  const [isThinkingDropdownOpen, setIsThinkingDropdownOpen] = useState(false);
  const [isApprovalDropdownOpen, setIsApprovalDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const thinkingDropdownRef = useRef<HTMLDivElement>(null);
  const approvalDropdownRef = useRef<HTMLDivElement>(null);
  // Real discovered models + the active chat model, loaded from the backend
  // settings (populated by API-key validation at login).
  const [models, setModels] = useState<string[]>([]);
  const [selectedModel, setSelectedModel] = useState<string>("");
  const [selectedThinking, setSelectedThinking] = useState<"simple" | "medium" | "deep">("medium");
  const [switching, setSwitching] = useState(false);
  const [slashResults, setSlashResults] = useState<SlashChoice[]>([]);
  const [slashSelected, setSlashSelected] = useState(0);

  const reloadSettings = useCallback((cancelled?: () => boolean) => {
    getSettings()
      .then((s) => {
        if (cancelled?.() || !s) return;
        setModels(s.available_models);
        setSelectedModel(s.chat_model);
        setSelectedThinking(s.thinking_depth ?? "medium");
      })
      .catch(() => {
        /* browser preview / uninitialized: leave the list empty */
      });
  }, []);

  useEffect(() => {
    let cancelled = false;
    reloadSettings(() => cancelled);
    const onSettingsChanged = () => reloadSettings();
    window.addEventListener(SETTINGS_CHANGED_EVENT, onSettingsChanged);
    return () => {
      cancelled = true;
      window.removeEventListener(SETTINGS_CHANGED_EVENT, onSettingsChanged);
    };
  }, [reloadSettings]);

  const chooseModel = async (id: string) => {
    setIsModelDropdownOpen(false);
    if (id === selectedModel || switching) return;
    const prev = selectedModel;
    setSelectedModel(id); // optimistic
    setSwitching(true);
    try {
      const view = await setChatModel(id);
      setSelectedModel(view.chat_model);
      message.success(t("composer.modelSwitched", { model: view.chat_model }));
    } catch (e) {
      setSelectedModel(prev); // revert on failure
      message.error(t("composer.modelSwitchFailed"));
      console.error("set_chat_model failed:", e);
    } finally {
      setSwitching(false);
    }
  };

  const THINKING_OPTIONS = [
    { id: "simple", label: "composer.thinkingSimple", icon: ["fas", "bolt"] as const },
    { id: "medium", label: "composer.thinkingMedium", icon: ["fas", "lightbulb"] as const },
    { id: "deep", label: "composer.thinkingDeep", icon: ["fas", "magnifying-glass"] as const },
  ] as const;
  const selectedThinkingOption =
    THINKING_OPTIONS.find((o) => o.id === selectedThinking) ?? THINKING_OPTIONS[1];

  const chooseThinking = async (id: "simple" | "medium" | "deep") => {
    setIsThinkingDropdownOpen(false);
    if (id === selectedThinking) return;
    const prev = selectedThinking;
    setSelectedThinking(id);
    try {
      const view = await setThinkingDepth(id);
      setSelectedThinking(view.thinking_depth ?? id);
    } catch (e) {
      setSelectedThinking(prev);
      message.error(t("composer.thinkingSwitchFailed"));
      console.error("set_thinking_depth failed:", e);
    }
  };

  const ALL_APPROVAL_OPTIONS = [
    { id: "default", label: "composer.defaultPermission", icon: ["fas", "hand"] as const },
    { id: "auto", label: "composer.autoReview", icon: ["fas", "clock-rotate-left"] as const },
    { id: "full", label: "composer.fullAccess", icon: ["fas", "circle-exclamation"] as const }
  ];
  
  const [visibleOptions, setVisibleOptions] = useState(ALL_APPROVAL_OPTIONS);
  const [selectedApproval, setSelectedApproval] = useState(ALL_APPROVAL_OPTIONS[0]);

  // Load the current backend approval policy and reflect it in the dropdown.
  useEffect(() => {
    let cancelled = false;
    getApprovalPolicy()
      .then((policy) => {
        if (cancelled) return;
        const optId = POLICY_TO_OPTION[policy] ?? "default";
        const opt = ALL_APPROVAL_OPTIONS.find((o) => o.id === optId);
        if (opt) setSelectedApproval(opt);
      })
      .catch(() => {
        /* browser preview / uninitialized */
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Apply the chosen permission level to the backend, then reflect it locally.
  const chooseApproval = async (opt: (typeof ALL_APPROVAL_OPTIONS)[number]) => {
    setIsApprovalDropdownOpen(false);
    const prev = selectedApproval;
    setSelectedApproval(opt); // optimistic
    try {
      await setApprovalPolicy(OPTION_TO_POLICY[opt.id] ?? "always_ask");
    } catch (e) {
      setSelectedApproval(prev); // revert on failure
      message.error(t("composer.approvalSwitchFailed"));
      console.error("set_approval_policy failed:", e);
    }
  };

  useEffect(() => {
    const updateVisibleOptions = () => {
      const showDefault = localStorage.getItem("approvalMenu_default") !== "false";
      const showAuto = localStorage.getItem("approvalMenu_auto") !== "false";
      const showFull = localStorage.getItem("approvalMenu_full") !== "false";
      
      const newOptions: typeof ALL_APPROVAL_OPTIONS[number][] = [];
      if (showDefault) newOptions.push(ALL_APPROVAL_OPTIONS[0]);
      if (showAuto) newOptions.push(ALL_APPROVAL_OPTIONS[1]);
      if (showFull) newOptions.push(ALL_APPROVAL_OPTIONS[2]);
      
      setVisibleOptions(newOptions);
      
      setSelectedApproval((prev) => {
        if (newOptions.find(o => o.id === prev.id)) return prev;
        return newOptions.length > 0 ? newOptions[0] : prev;
      });
    };

    updateVisibleOptions();
    window.addEventListener("approvalMenuChanged", updateVisibleOptions);
    return () => window.removeEventListener("approvalMenuChanged", updateVisibleOptions);
  }, []);

  // The active model's two-tier label (empty until settings load).
  const selectedLabel = labelFor(selectedModel);

  const slashOpen = value.startsWith("/") && slashResults.length > 0;

  useEffect(() => {
    if (!value.startsWith("/")) {
      setSlashResults([]);
      setSlashSelected(0);
      return;
    }
    const body = value.slice(1);
    const firstSpace = body.search(/\s/);
    const commandName = firstSpace >= 0 ? body.slice(0, firstSpace) : body;
    const argQuery = firstSpace >= 0 ? body.slice(firstSpace + 1).trim().toLowerCase() : "";
    const exactArgCommand = firstSpace < 0 && (commandName === "model" || commandName === "thinking");

    if (commandName === "model" && (firstSpace >= 0 || exactArgCommand)) {
      const choices = models
        .filter((id) => id.toLowerCase().includes(argQuery))
        .map((id) => ({
          id: `slash-arg.model.${id}`,
          title: id,
          description: id === selectedModel ? "当前模型" : "切换到这个 DeepSeek 模型",
          category: "模型",
          shortcut: null,
          insertText: `/model ${id} `,
        }));
      setSlashResults(choices);
      setSlashSelected(0);
      return;
    }

    if (commandName === "thinking" && (firstSpace >= 0 || exactArgCommand)) {
      const choices = [
        { id: "simple", title: "simple", description: "简单思考，响应更快", category: "思考" },
        { id: "medium", title: "medium", description: "中度思考，默认平衡模式", category: "思考" },
        { id: "deep", title: "deep", description: "深度思考，给复杂任务更多推理预算", category: "思考" },
      ]
        .filter((opt) => opt.title.includes(argQuery) || opt.description.includes(argQuery))
        .map((opt) => ({
          ...opt,
          id: `slash-arg.thinking.${opt.id}`,
          shortcut: null,
          insertText: `/thinking ${opt.id} `,
        }));
      setSlashResults(choices);
      setSlashSelected(0);
      return;
    }

    if (firstSpace >= 0) {
      setSlashResults([]);
      setSlashSelected(0);
      return;
    }

    let cancelled = false;
    getCommands("")
      .then((commands) => {
        if (cancelled) return;
        const q = body.trim().toLowerCase();
        const slash = commands.filter((c) => {
          if (!c.id.startsWith("slash.")) return false;
          const name = c.title.replace(/^\//, "").toLowerCase();
          return !q || name.includes(q);
        });
        setSlashResults(slash);
        setSlashSelected(0);
      })
      .catch(() => {
        if (!cancelled) setSlashResults([]);
      });
    return () => {
      cancelled = true;
    };
  }, [models, selectedModel, value]);

  const chooseSlash = (cmd: SlashChoice) => {
    onChange(cmd.insertText ?? `${cmd.title} `);
    setSlashResults([]);
    setSlashSelected(0);
  };

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsModelDropdownOpen(false);
      }
      if (thinkingDropdownRef.current && !thinkingDropdownRef.current.contains(e.target as Node)) {
        setIsThinkingDropdownOpen(false);
      }
      if (approvalDropdownRef.current && !approvalDropdownRef.current.contains(e.target as Node)) {
        setIsApprovalDropdownOpen(false);
      }
    };
    if (isModelDropdownOpen || isThinkingDropdownOpen || isApprovalDropdownOpen) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [isModelDropdownOpen, isThinkingDropdownOpen, isApprovalDropdownOpen]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (slashOpen && e.key === "ArrowDown") {
      e.preventDefault();
      setSlashSelected((s) => Math.min(s + 1, slashResults.length - 1));
      return;
    }
    if (slashOpen && e.key === "ArrowUp") {
      e.preventDefault();
      setSlashSelected((s) => Math.max(s - 1, 0));
      return;
    }
    if (slashOpen && (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey))) {
      const selected = slashResults[slashSelected];
      const exact = selected?.title === value.trim();
      if (selected && !exact) {
        e.preventDefault();
        chooseSlash(selected);
        return;
      }
    }
    if (slashOpen && e.key === "Escape") {
      e.preventDefault();
      setSlashResults([]);
      return;
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (!busy) onSubmit();
    }
  };

  return (
    <div className="relative w-full border border-border-theme rounded-2xl shadow-[0_2px_10px_rgba(0,0,0,0.02)] bg-white flex flex-col transition-all focus-within:border-gray-300 focus-within:shadow-md">
      <div className="p-3 pb-2 flex flex-col relative w-full">
      {slashOpen && (
        <div className="absolute left-3 right-3 bottom-full mb-2 max-h-72 overflow-y-auto rounded-lg border border-border-theme bg-white py-1 shadow-[0_8px_30px_rgb(0,0,0,0.12)] z-50">
          {slashResults.map((cmd, index) => (
            <button
              key={cmd.id}
              type="button"
              onMouseDown={(e) => {
                e.preventDefault();
                chooseSlash(cmd);
              }}
              onMouseEnter={() => setSlashSelected(index)}
              className={`grid w-full grid-cols-[150px_minmax(0,1fr)_72px] items-center gap-3 px-3 py-2 text-left text-[12px] transition-colors ${
                index === slashSelected ? "bg-gray-100 text-text-base" : "text-text-secondary"
              }`}
            >
              <span className="truncate font-medium text-text-base">{cmd.title}</span>
              <span className="truncate text-[12px] leading-snug text-text-secondary">
                {cmd.description}
              </span>
              <span className="truncate text-right text-[11px] text-text-secondary">{cmd.category}</span>
            </button>
          ))}
        </div>
      )}
      {planMode && (
        <div className="mb-2 inline-flex w-fit items-center rounded-md border border-amber-200 bg-amber-50 px-2 py-1 text-[11px] font-medium text-amber-700">
          <FontAwesomeIcon icon={["fas", "list-check"]} className="mr-1.5 text-[10px]" />
          Plan Mode
        </div>
      )}
      <textarea
        className="w-full min-h-[60px] max-h-[200px] text-text-base placeholder-gray-400 text-sm bg-transparent"
        placeholder={placeholder ?? t("composer.placeholder")}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
      />

      <div className="flex items-center justify-between mt-2 pt-1">
        <div className="flex items-center space-x-2">
          <button className="w-7 h-7 rounded flex items-center justify-center text-text-secondary hover:bg-gray-100 transition-colors">
            <FontAwesomeIcon icon={["fas", "plus"]} />
          </button>
          
          {visibleOptions.length > 0 && (
            <div className="relative" ref={approvalDropdownRef}>
              <div 
                className="flex items-center text-blue-500 text-xs font-medium cursor-pointer hover:bg-blue-50 px-2 py-1.5 rounded transition-colors"
                onClick={() => setIsApprovalDropdownOpen(!isApprovalDropdownOpen)}
              >
                {selectedApproval && (
                  <>
                    <FontAwesomeIcon
                      icon={selectedApproval.icon as any}
                      className="mr-1.5"
                    />
                    {t(selectedApproval.label)}
                  </>
                )}
                <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-1 text-[10px]" />
              </div>

              {/* Approval Dropdown */}
              {isApprovalDropdownOpen && (
                <div className="absolute bottom-full left-0 mb-2 w-[160px] bg-white border border-border-theme rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] flex flex-col z-50 overflow-hidden py-1">
                  {visibleOptions.map((opt) => (
                    <div
                      key={opt.id}
                      className="flex items-center justify-between px-3 py-2 hover:bg-gray-100 cursor-pointer text-xs text-text-base group transition-colors"
                      onClick={() => chooseApproval(opt)}
                    >
                      <div className="flex items-center">
                        <div className="w-5 flex justify-center mr-1">
                          <FontAwesomeIcon icon={opt.icon as any} className="text-text-secondary" />
                        </div>
                        <span className="font-medium text-text-secondary group-hover:text-text-base transition-colors">{t(opt.label)}</span>
                      </div>
                      {selectedApproval?.id === opt.id && (
                        <FontAwesomeIcon icon={["fas", "check"]} className="text-text-base text-[10px]" />
                      )}
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
        </div>

        <div className="flex items-center space-x-2">
          <div className="relative" ref={thinkingDropdownRef}>
            <div
              className="flex items-center bg-gray-50 border border-border-theme rounded-full px-2.5 py-1 cursor-pointer hover:bg-gray-100 transition-colors text-xs text-text-base"
              onClick={() => setIsThinkingDropdownOpen(!isThinkingDropdownOpen)}
              title={t("composer.selectThinking")}
            >
              <FontAwesomeIcon icon={selectedThinkingOption.icon as any} className="text-text-secondary" />
              <span className="ml-1.5 text-text-secondary">{t(selectedThinkingOption.label)}</span>
              <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-2 text-[10px] text-text-secondary" />
            </div>

            {isThinkingDropdownOpen && (
              <div className="absolute bottom-full right-0 mb-2 w-full bg-white border border-border-theme rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] flex flex-col z-50 overflow-hidden py-1">
                <div className="px-3 py-2 text-[11px] text-text-secondary font-medium">{t("composer.selectThinking")}</div>
                {THINKING_OPTIONS.map((opt) => (
                  <div
                    key={opt.id}
                    className="flex items-center justify-between px-3 py-2 hover:bg-gray-100 cursor-pointer text-[13px] text-text-base group transition-colors"
                    onClick={() => chooseThinking(opt.id)}
                  >
                    <div className="flex items-center">
                      <FontAwesomeIcon icon={opt.icon as any} className="w-4 text-text-secondary mr-2" />
                      <span className="font-medium">{t(opt.label)}</span>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
          <div className="relative" ref={dropdownRef}>
            <div 
              className="flex items-center bg-gray-50 border border-border-theme rounded-full px-3 py-1 cursor-pointer hover:bg-gray-100 transition-colors text-xs text-text-base"
              onClick={() => setIsModelDropdownOpen(!isModelDropdownOpen)}
            >
              {selectedModel ? (
                <>
                  {selectedLabel.name} {selectedLabel.version && <span className="text-text-secondary ml-1.5">{selectedLabel.version}</span>}
                </>
              ) : (
                <span className="text-text-secondary">{t("composer.selectModel")}</span>
              )}
              <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-2 text-[10px] text-text-secondary" />
            </div>

            {/* Model Dropdown */}
            {isModelDropdownOpen && (
              <div className="absolute bottom-full right-0 mb-2 w-full bg-white border border-border-theme rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] flex flex-col z-50 overflow-hidden py-1">
                <div className="px-3 py-2 text-[11px] text-text-secondary font-medium">{t("composer.selectModel")}</div>
                <div className="flex-1 max-h-[240px] overflow-y-auto py-1">
                  {models.length === 0 && (
                    <div className="px-4 py-2 text-[12px] text-text-secondary">
                      {t("composer.noModels")}
                    </div>
                  )}
                  {models.map((id) => {
                    const lbl = labelFor(id);
                    return (
                      <div
                        key={id}
                        className="flex items-center justify-between px-3 py-2 hover:bg-gray-100 cursor-pointer text-[13px] text-text-base group transition-colors"
                        onClick={() => chooseModel(id)}
                      >
                        <div className="flex items-center">
                          <span className="font-medium">{lbl.name}</span>
                          {lbl.version && <span className="text-text-secondary ml-1.5 text-[12px]">{lbl.version}</span>}
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>
            )}
          </div>
          <button
            onClick={() => {
              if (busy) {
                onStop?.();
              } else {
                onSubmit();
              }
            }}
            disabled={busy && !onStop}
            title={busy ? t("composer.stop") : undefined}
            className={`w-8 h-8 rounded-full text-white flex items-center justify-center transition-colors ${
              busy
                ? onStop
                  ? "bg-text-base hover:bg-red-500 cursor-pointer"
                  : "bg-gray-300 cursor-not-allowed"
                : planMode
                ? "bg-amber-500 hover:bg-amber-600 cursor-pointer"
                : "bg-gray-400 hover:bg-primary cursor-pointer"
            }`}
          >
            <FontAwesomeIcon icon={busy ? ["fas", "stop"] : ["fas", "arrow-up"]} />
          </button>
        </div>
      </div>
      </div>
      
      {footer && (
        <div className="w-full bg-gray-50 border-t border-border-theme px-3 py-1.5 flex items-center min-h-[32px] rounded-b-2xl">
          {footer}
        </div>
      )}
    </div>
  );
}
