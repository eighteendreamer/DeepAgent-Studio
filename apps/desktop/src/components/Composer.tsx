import { HoverInfo } from "./ui/HoverInfo";
import { useState, useRef, useEffect, useCallback, useLayoutEffect, useMemo, useId } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Brain, BrainCircuit, Check, ChevronDown, CircleAlert, Feather, Hand, Lightbulb, Layers, ListChecks, Loader2, Puzzle, ShieldHalf, Sparkles, createLucideIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { ContextCapacityIndicator } from "./ContextCapacityIndicator";
import { ComposerSuggestPanel } from "./ComposerSuggestPanel";
import { ModelThinkingSelector } from "./ModelThinkingSelector";
import { useComposerModelSettings, type ThinkingLevel } from "../hooks/useComposerModelSettings";
import { InputSurface } from "./ui/InputSurface";
import { useMessageSendMotion } from "./ui/MessageSendMotionContext";
import { ComposerAttachButton } from "./ComposerAttachButton";
import { ComposerAttachmentList } from "./ComposerAttachmentList";
import { validateComposerImage } from "./composerImageInput";
import { findCommittedSlash, projectSlashDraft, restoreSlashDraft, slashCommandKind, toSlashEditorOffset } from "./composerSlashToken";
import { MorphingMenuShell } from "./ui/MorphingMenuShell";
import { MOTION } from "./ui/motion";
import { MENU_ITEM_ATTR, SlidingMenuList } from "./ui/SlidingMenuList";
import { Button } from "./shadcn/button";
import { cn } from "./shadcn/utils";
import {
  PLUGINS_CHANGED_EVENT,
  SETTINGS_CHANGED_EVENT,
  attachmentIngest,
  attachmentRemove,
  getActivePermissionPreset,
  getCommands,
  getPermissionPresetVisibility,
  listPlugins,
  listSkills,
  searchProjectFiles,
  setActivePermissionPreset,
} from "../api";
import type { SettingsChangedDetail } from "../api";
import { message } from "./message";
import type {
  Command,
  ComposerAttachment,
  ComposerMention,
  ComposerSkillSelection,
  ContextUsageSnapshot,
  PluginSummary,
  ProjectFileEntry,
  Skill,
} from "../types";

const LONG_TEXT_ATTACHMENT_THRESHOLD = 8 * 1024;
const MAX_COMPOSER_ATTACHMENTS = 5;
const COMPOSER_TEXTAREA_MIN_HEIGHT = 60;
const COMPOSER_TEXTAREA_DEFAULT_MAX_HEIGHT = 300;
const SKILL_MARKER = "\uE000";
const MENTION_MARKER = "\uE001";
const GoalTarget = createLucideIcon("goal-target", [
  ["path", { d: "M21 12a9 9 0 1 1-9-9m5 9a5 5 0 1 1-5-5", key: "rings" }],
  ["path", { d: "M20 4l-8 8m0-4v4h4", key: "arrow" }],
]);

/** A slash-dropdown row. `insertText` is used for built-in slash commands;
 *  `skillName` / `skillId` mark a row as a skill picker. */
type SlashChoice = Command & { insertText?: string; skillId?: string; skillName?: string };
type SlashSection = {
  key: string;
  label: string;
  items: SlashChoice[];
};
type SlashToken = {
  start: number;
  end: number;
  query: string;
};
type AtToken = SlashToken;
type InlineMention = Exclude<ComposerMention, { kind: "goal" | "plan_mode" }>;
type SelectionRange = {
  start: number;
  end: number;
};
type AtChoice =
  | {
      id: "add-files";
      type: "add-files";
      title: string;
      description: string;
      icon: "paperclip";
    }
  | {
      id: "goal";
      type: "goal";
      title: string;
      description: string;
    }
  | {
      id: "plan-mode";
      type: "plan-mode";
      title: string;
      description: string;
    }
  | {
      id: string;
      type: "file";
      title: string;
      description: string;
      icon: "file" | "folder";
      entry: ProjectFileEntry;
    }
  | {
      id: string;
      type: "plugin";
      title: string;
      description: string;
      pluginId: string;
    };
type AtSection = {
  key: string;
  label: string;
  items: AtChoice[];
};

/** Map composer dropdown option id -> backend permission preset label. */
const OPTION_TO_PRESET: Record<string, string> = {
  default: "default",
  auto: "auto_review",
  full: "full_access",
};
const PRESET_TO_OPTION: Record<string, string> = {
  default: "default",
  auto_review: "auto",
  full_access: "full",
};

function stripSkillMarkers(value: string): string {
  return value.replace(/[\uE000\uE001]/g, "");
}

function countSkillMarkers(value: string): number {
  return (value.match(/\uE000/g) ?? []).length;
}

function countMentionMarkers(value: string): number {
  return (value.match(/\uE001/g) ?? []).length;
}

function normalizeSelectionRange(start: number, end: number): SelectionRange | null {
  if (start === end) return null;
  return {
    start: Math.min(start, end),
    end: Math.max(start, end),
  };
}

function rangesOverlap(range: SelectionRange | null, start: number, end: number): boolean {
  return Boolean(range && start < range.end && end > range.start);
}

function findActiveSlashToken(value: string, cursor: number): SlashToken | null {
  const beforeCursor = value.slice(0, cursor);
  const match = beforeCursor.match(/(^|[\s\uE000\uE001])\/([a-zA-Z0-9_:-]*)$/);
  if (!match || match.index === undefined) return null;
  const start = match.index + match[1].length;
  const after = value.slice(start + 1);
  const fullCommand = after.match(/^[a-zA-Z0-9_:-]*/)?.[0] ?? "";
  if (cursor > start + 1 + fullCommand.length) return null;
  return {
    start,
    end: start + 1 + fullCommand.length,
    query: fullCommand,
  };
}

function findActiveAtToken(value: string, cursor: number): AtToken | null {
  const beforeCursor = value.slice(0, cursor);
  const match = beforeCursor.match(/(^|[\s\uE000\uE001])@([^\s\uE000\uE001]*)$/);
  if (!match || match.index === undefined) return null;
  const start = match.index + match[1].length;
  const after = value.slice(start + 1);
  const fullMention = after.match(/^[^\s\uE000\uE001]*/)?.[0] ?? "";
  if (cursor > start + 1 + fullMention.length) return null;
  return {
    start,
    end: start + 1 + fullMention.length,
    query: fullMention,
  };
}

interface Props {
  value: string;
  onChange: (v: string) => void;
  onSubmit: (
    attachments?: ComposerAttachment[],
    selectedSkills?: ComposerSkillSelection[],
    mentions?: ComposerMention[],
    displayText?: string,
  ) => void;
  placeholder?: string;
  /** True while a run is streaming: disables submit and shows a busy button. */
  busy?: boolean;
  /** Stop the in-flight run (turns the busy button into a stop button). */
  onStop?: () => void;
  /** Cancellation was accepted and the backend is draining the run. */
  stopping?: boolean;
  /** True when the current session is in read-only Plan mode. */
  planMode?: boolean | null;
  onExitPlanMode?: () => void;
  /** Active project path used for @ file/folder search. */
  activeProjectPath?: string | null;
  /** Optional footer content rendered seamlessly at the bottom of the composer. */
  footer?: React.ReactNode;
  /** Latest backend context-usage snapshot for the current run/session. */
  contextUsage?: ContextUsageSnapshot | null;
  /** Fallback prompt tokens from persisted/live usage when context_usage was missed. */
  contextUsageFallbackTokens?: number;
  /** Maximum auto-expanded textarea height in pixels. */
  textareaMaxHeight?: number;
  /** Increment to force-close composer toolbar overlays (model / approval). */
  overlayCloseSignal?: number;
  /** Fired when any composer toolbar overlay opens or closes. */
  onOverlayOpenChange?: (open: boolean) => void;
}

export function Composer({
  value,
  onChange,
  onSubmit,
  placeholder,
  busy = false,
  onStop,
  stopping = false,
  planMode = false,
  onExitPlanMode,
  activeProjectPath = null,
  footer,
  contextUsage = null,
  contextUsageFallbackTokens = 0,
  textareaMaxHeight = COMPOSER_TEXTAREA_DEFAULT_MAX_HEIGHT,
  overlayCloseSignal = 0,
  onOverlayOpenChange,
}: Props) {
  const { t } = useTranslation();
  const attachmentLimitMessage = t("composer.attachments.limit", {
    count: MAX_COMPOSER_ATTACHMENTS,
  });
  const [isModelDropdownOpen, setIsModelDropdownOpen] = useState(false);
  const [isApprovalDropdownOpen, setIsApprovalDropdownOpen] = useState(false);
  const [isContextPopoverOpen, setIsContextPopoverOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const approvalDropdownRef = useRef<HTMLDivElement>(null);
  const approvalMorphLayoutId = useId().replace(/:/g, "");
  const suggestId = useId();
  const lastOverlayCloseSignal = useRef(overlayCloseSignal);
  const lastReportedOverlayOpen = useRef(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const mirrorRef = useRef<HTMLDivElement>(null);
  const slashChipRef = useRef<HTMLSpanElement>(null);
  const previousSlashPrefix = useRef("");
  const lastCommittedSlashPrefix = useRef("");
  const composerRef = useRef<HTMLDivElement>(null);
  const messageSendMotion = useMessageSendMotion();
  const [attachments, setAttachments] = useState<ComposerAttachment[]>([]);
  const { modelOptions, activeModelOptions, selectedModel, selectedThinking, switching,
    chooseModel, chooseThinking } = useComposerModelSettings();
  const [slashResults, setSlashResults] = useState<SlashChoice[]>([]);
  const [slashCommands, setSlashCommands] = useState<Command[]>([]);
  const [slashSelected, setSlashSelected] = useState(0);
  const [suggestionNavigation, setSuggestionNavigation] = useState(0);
  const [atSections, setAtSections] = useState<AtSection[]>([]);
  const [atSelected, setAtSelected] = useState(0);
  const [selectedSkills, setSelectedSkills] = useState<ComposerSkillSelection[]>([]);
  const [selectedMentions, setSelectedMentions] = useState<InlineMention[]>([]);
  const [draftMode, setDraftMode] = useState<"goal" | "plan" | null>(null);
  const mode = draftMode ?? (planMode === true ? "plan" : null);
  const [plugins, setPlugins] = useState<PluginSummary[]>([]);
  const [draftValue, setDraftValue] = useState(value);
  const [cursorPos, setCursorPos] = useState(value.length);
  const [selectionRange, setSelectionRange] = useState<SelectionRange | null>(null);
  const [editorFocused, setEditorFocused] = useState(false);
  const pendingDraftValueRef = useRef(value);
  // Skill registry mirror, used to populate slash candidates (Channel C v1).
  // Reloads on `SETTINGS_CHANGED_EVENT` so install/uninstall mid-session is
  // reflected without a chat-page refresh.
  const [skills, setSkills] = useState<Skill[]>([]);

  // Load the skill registry for the slash-command candidate list, and refresh
  // when settings change (catches install / uninstall / reload mid-session).
  useEffect(() => {
    let cancelled = false;
    const load = () => {
      listSkills()
        .then((list) => {
          if (!cancelled) setSkills(list);
        })
        .catch(() => {
          /* browser preview / uninitialized: keep prior list */
        });
    };
    load();
    const onSettingsChanged = (event: Event) => {
      const reason = (event as CustomEvent<SettingsChangedDetail>).detail?.reason;
      if (!reason || ["skill_catalog", "skill_ai_review", "slash_settings", "other"].includes(reason)) load();
    };
    window.addEventListener(SETTINGS_CHANGED_EVENT, onSettingsChanged);
    return () => {
      cancelled = true;
      window.removeEventListener(SETTINGS_CHANGED_EVENT, onSettingsChanged);
    };
  }, []);

  useEffect(() => {
    if (value === pendingDraftValueRef.current) return;
    if (stripSkillMarkers(draftValue) === value) {
      pendingDraftValueRef.current = value;
      return;
    }
    setDraftValue(value);
    setCursorPos(value.length);
    setSelectionRange(null);
    setSlashResults([]);
    setSlashSelected(0);
    setSelectedSkills([]);
    setAtSections([]);
    setAtSelected(0);
    setSelectedMentions([]);
    setDraftMode(null);
    pendingDraftValueRef.current = value;
  }, [draftValue, value]);

  const THINKING_OPTIONS = [
    { id: "off", label: "composer.thinkingNone", icon: Feather },
    { id: "low", label: "composer.thinkingLow", icon: Lightbulb },
    { id: "medium", label: "composer.thinkingMedium", icon: Brain },
    { id: "high", label: "composer.thinkingHigh", icon: BrainCircuit },
    { id: "max", label: "composer.thinkingMax", icon: Sparkles },
  ] as const satisfies readonly { id: ThinkingLevel; label: string; icon: typeof Feather }[];

  const ALL_APPROVAL_OPTIONS = [
    { id: "default", label: "composer.defaultPermission", desc: "composer.defaultPermissionDesc", icon: Hand },
    { id: "auto", label: "composer.autoReview", desc: "composer.autoReviewDesc", icon: ShieldHalf },
    { id: "full", label: "composer.fullAccess", desc: "composer.fullAccessDesc", icon: CircleAlert },
  ];

  const [visibleOptions, setVisibleOptions] = useState(ALL_APPROVAL_OPTIONS);
  const [selectedApproval, setSelectedApproval] = useState(ALL_APPROVAL_OPTIONS[0]);

  // Load the current backend permission preset and reflethe dropdown.
  useEffect(() => {
    let cancelled = false;
    getActivePermissionPreset()
      .then((preset) => {
        if (cancelled) return;
        const optId = PRESET_TO_OPTION[preset] ?? "default";
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

  // Apply the chosen permission preset to the backend, then reflect it locally.
  const chooseApproval = async (opt: (typeof ALL_APPROVAL_OPTIONS)[number]) => {
    setIsApprovalDropdownOpen(false);
    const prev = selectedApproval;
    setSelectedApproval(opt); // optimistic
    try {
      await setActivePermissionPreset((OPTION_TO_PRESET[opt.id] ?? "default") as any);
    } catch (e) {
      setSelectedApproval(prev); // revert on failure
      message.error(t("composer.approvalSwitchFailed"));
      console.error("set_active_permission_preset failed:", e);
    }
  };

  useEffect(() => {
    const updateVisibleOptions = async () => {
      try {
        const vis = await getPermissionPresetVisibility();
        const newOptions: typeof ALL_APPROVAL_OPTIONS[number][] = [];
        if (vis.default_enabled) newOptions.push(ALL_APPROVAL_OPTIONS[0]);
        if (vis.auto_review_enabled) newOptions.push(ALL_APPROVAL_OPTIONS[1]);
        if (vis.full_access_enabled) newOptions.push(ALL_APPROVAL_OPTIONS[2]);

        setVisibleOptions(newOptions);

        setSelectedApproval((prev) => {
          if (newOptions.find((o) => o.id === prev.id)) return prev;
          return newOptions.length > 0 ? newOptions[0] : prev;
        });
      } catch {
        /* fallback: show all */
      }
    };

    updateVisibleOptions();
    window.addEventListener("approvalMenuChanged", updateVisibleOptions);
    return () => window.removeEventListener("approvalMenuChanged", updateVisibleOptions);
  }, []);

  const slashSections = useMemo<SlashSection[]>(() => {
    const systemCommands = slashResults.filter((cmd) => cmd.skillName === undefined);
    const skillsGroup = slashResults.filter((cmd) => cmd.skillName !== undefined);
    const sections: SlashSection[] = [];
    if (systemCommands.length > 0) {
      sections.push({
        key: "system",
        label: t("composer.systemCommands", { defaultValue: "System Commands" }),
        items: systemCommands,
      });
    }
    if (skillsGroup.length > 0) {
      sections.push({
        key: "skills",
        label: t("composer.skillsGroup", { defaultValue: "Skills" }),
        items: skillsGroup,
      });
    }
    return sections;
  }, [slashResults, t]);

  const slashEntries = useMemo(
    () => slashSections.flatMap((section) => section.items),
    [slashSections],
  );

  const atEntries = useMemo(
    () => atSections.flatMap((section) => section.items),
    [atSections],
  );

  const activeSlashToken = useMemo(
    () => findActiveSlashToken(draftValue, cursorPos),
    [cursorPos, draftValue],
  );

  const hasSlashInput = draftValue.startsWith("/") || activeSlashToken !== null;
  useEffect(() => {
    if (!hasSlashInput) return;
    let currentRequest = 0;
    const refresh = () => {
      const request = ++currentRequest;
      getCommands("")
        .then((commands) => {
          if (request === currentRequest) setSlashCommands(commands.filter((command) => command.id.startsWith("slash.")));
        })
        .catch(() => {
          // Keep the last catalog while a refresh is unavailable.
        });
    };
    refresh();
    window.addEventListener(SETTINGS_CHANGED_EVENT, refresh);
    return () => {
      currentRequest += 1;
      window.removeEventListener(SETTINGS_CHANGED_EVENT, refresh);
    };
  }, [activeProjectPath, hasSlashInput]);

  const slashTitles = useMemo(() => new Set(slashCommands.map((command) => command.title)), [slashCommands]);
  const committedSlash = findCommittedSlash(draftValue, slashTitles);
  const slashPrefix = committedSlash?.prefix ?? "";
  const editorValue = projectSlashDraft(draftValue, slashPrefix);

  // Only adjust the native selection when the editor switches between the raw
  // command and its one-character representation. Ordinary text edits stay native.
  useLayoutEffect(() => {
    if (previousSlashPrefix.current === slashPrefix) return;
    previousSlashPrefix.current = slashPrefix;
    if (slashPrefix) lastCommittedSlashPrefix.current = slashPrefix;
    textareaRef.current?.setSelectionRange(
      toSlashEditorOffset(cursorPos, slashPrefix),
      toSlashEditorOffset(selectionRange?.end ?? cursorPos, slashPrefix),
    );
  }, [cursorPos, selectionRange, slashPrefix]);

  const focusEditorAt = (nextDraft: string, caret: number) => {
    const prefix = findCommittedSlash(nextDraft, slashTitles)?.prefix ?? "";
    const editorCaret = toSlashEditorOffset(caret, prefix);
    requestAnimationFrame(() => {
      textareaRef.current?.setSelectionRange(editorCaret, editorCaret);
      textareaRef.current?.focus();
    });
  };

  const activeAtToken = useMemo(
    () => findActiveAtToken(draftValue, cursorPos),
    [cursorPos, draftValue],
  );

  const slashOpen = Boolean((draftValue.startsWith("/") || activeSlashToken) && slashEntries.length > 0);
  const atOpen = Boolean(!slashOpen && activeAtToken && atEntries.length > 0);
  const hasAtToken = activeAtToken !== null;
  useEffect(() => {
    if (!hasAtToken) return;
    let currentRequest = 0;
    const refresh = () => {
      const request = ++currentRequest;
      void listPlugins().then((rows) => {
        if (request === currentRequest) setPlugins(rows);
      }).catch((error) => {
        if (request === currentRequest) {
          setPlugins([]);
          console.error("Failed to load composer plugins", error);
        }
      });
    };
    refresh();
    window.addEventListener(PLUGINS_CHANGED_EVENT, refresh);
    return () => {
      currentRequest += 1;
      window.removeEventListener(PLUGINS_CHANGED_EVENT, refresh);
    };
  }, [hasAtToken]);

  const resizeTextarea = useCallback(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;

    // An empty draft is always exactly the minimum. Going through a
    // measurement here is what let a stale read persist as an oversized box
    // that only shrank once the user typed again.
    if (!textarea.value) {
      textarea.style.height = `${COMPOSER_TEXTAREA_MIN_HEIGHT}px`;
      textarea.style.overflowY = "hidden";
      return;
    }

    // The mirror shares the editor's width and wrapping, but keeps its natural
    // content height (no bottom inset). Stretching it to the textarea's viewport
    // would feed the previous height back into this measurement and prevent
    // shorter drafts from shrinking.
    const mirror = mirrorRef.current;
    const measured = mirror ? mirror.scrollHeight : textarea.scrollHeight;
    const nextHeight = Math.min(
      Math.max(measured, COMPOSER_TEXTAREA_MIN_HEIGHT),
      textareaMaxHeight,
    );
    textarea.style.height = `${nextHeight}px`;
    textarea.style.overflowY = measured > textareaMaxHeight ? "auto" : "hidden";
  }, [textareaMaxHeight]);

  useLayoutEffect(() => {
    resizeTextarea();
  }, [draftValue, resizeTextarea, slashPrefix]);

  // The mirror lays out the chip at its natural width. Reserve exactly that
  // space on the textarea's first line so wrapped arguments stay aligned.
  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    const chip = slashChipRef.current;
    if (!textarea || !chip || !slashPrefix) return;
    const syncIndent = () => {
      textarea.style.textIndent = `${chip.getBoundingClientRect().width}px`;
    };
    syncIndent();
    const observer = new ResizeObserver(syncIndent);
    observer.observe(chip);
    return () => {
      observer.disconnect();
      textarea.style.removeProperty("text-indent");
    };
  }, [slashPrefix]);

  // Width is the other input to the measurement, and it changes without
  // touching the draft: window resize, sidebar toggle, moving the window to a
  // display with a different scale factor. Recompute on width only — observing
  // the height we just wrote would loop — and wait a frame so a transition in
  // flight is not what gets measured.
  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;

    let frame: number | null = null;
    let lastWidth = textarea.clientWidth;
    const observer = new ResizeObserver(() => {
      if (textarea.clientWidth === lastWidth) return;
      lastWidth = textarea.clientWidth;
      if (frame !== null) cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        frame = null;
        resizeTextarea();
      });
    });
    observer.observe(textarea);
    return () => {
      observer.disconnect();
      if (frame !== null) cancelAnimationFrame(frame);
    };
  }, [resizeTextarea]);

  useEffect(() => {
    if (draftValue.startsWith("/")) {
      const body = draftValue.slice(1);
      const firstSpace = body.search(/\s/);
      const commandName = firstSpace >= 0 ? body.slice(0, firstSpace) : body;
      const argQuery = firstSpace >= 0 ? body.slice(firstSpace + 1).trim().toLowerCase() : "";
      const exactArgCommand = firstSpace < 0 && (commandName === "model" || commandName === "thinking");

      if (commandName === "model" && (firstSpace >= 0 || exactArgCommand)) {
        const choices = activeModelOptions
          .filter((model) => `${model.modelName} ${model.providerLabel}`.toLowerCase().includes(argQuery))
          .map((model) => ({
            id: `slash-arg.model.${model.id}`,
            title: model.modelName,
            description: model.id === selectedModel ? `当前模型 · ${model.providerLabel}` : `切换到 ${model.providerLabel} 的模型`,
            category: "模型",
            shortcut: null,
            insertText: `/model ${model.providerId}/${model.modelName} `,
          }));
        setSlashResults(choices);
        setSlashSelected(0);
        return;
      }

      if (commandName === "thinking" && (firstSpace >= 0 || exactArgCommand)) {
        const choices = [
          { id: "off", title: "off", description: "关闭推理", category: "推理强度" },
          { id: "low", title: "low", description: "低推理强度", category: "推理强度" },
          { id: "medium", title: "medium", description: "中推理强度", category: "推理强度" },
          { id: "high", title: "high", description: "高推理强度", category: "推理强度" },
          { id: "max", title: "max", description: "最高推理强度", category: "推理强度" },
        ]
          .filter(opt => activeModelOptions.find(model => model.id === selectedModel)?.thinkingLevels.includes(opt.id))
          .filter((opt) => opt.title.includes(argQuery) || opt.description.includes(argQuery))
          .map((opt) => ({
            ...opt,
            id: `slash-arg.thinking.${opt.id}`,
            shortcut: null,
            insertText: `/thinking ${opt.id === "off" ? "none" : opt.id} `,
          }));
        setSlashResults(choices);
        setSlashSelected(0);
        return;
      }
    }

    if (!activeSlashToken) {
      setSlashResults([]);
      setSlashSelected(0);
      return;
    }

    const q = activeSlashToken.query.trim().toLowerCase();
    const selectedSkillIds = new Set(selectedSkills.map((skill) => skill.id));
    const skillChoices: SlashChoice[] = skills
      .filter(
        (sk) =>
          !selectedSkillIds.has(sk.id) &&
          (!q ||
            sk.id.toLowerCase().includes(q) ||
            sk.name.toLowerCase().includes(q)),
      )
      .sort((a, b) => a.id.localeCompare(b.id))
      .slice(0, 8)
      .map((sk) => ({
        id: `slash-skill.${sk.id}`,
        title: `/${sk.id}`,
        description: sk.description,
        category: "Skill",
        shortcut: null,
        skillId: sk.id,
        skillName: sk.name,
      }));
    const slash = slashCommands.filter((command) =>
      slashCommandKind(command.title) !== "plugin" && (!q || command.title.slice(1).toLowerCase().includes(q)),
    );
    setSlashResults([...slash, ...skillChoices]);
    setSlashSelected(0);
  }, [activeSlashToken, draftValue, activeModelOptions, selectedModel, selectedSkills, skills, slashCommands]);

  useEffect(() => {
    if (!activeAtToken || slashOpen) {
      setAtSections([]);
      setAtSelected(0);
      return;
    }

    let cancelled = false;
    const q = activeAtToken.query.trim().toLowerCase();
    const matches = (value: string) => !q || value.toLowerCase().includes(q);
    const sections: AtSection[] = [];
    const addChoices: AtChoice[] = [
      {
        id: "add-files",
        type: "add-files",
        title: "文件和文件夹",
        description: q ? "继续输入以搜索项目文件和文件夹" : "",
        icon: "paperclip",
      },
      {
        id: "goal",
        type: "goal",
        title: t("composer.goal.title"),
        description: t("composer.goal.description"),
      },
      {
        id: "plan-mode",
        type: "plan-mode",
        title: t("composer.plan.title"),
        description: t("composer.plan.description"),
      },
    ];
    const addItems = addChoices.filter((item) => matches(item.title) || matches(item.description));
    if (addItems.length > 0) {
      sections.push({ key: "add", label: "添加", items: addItems });
    }

    const selectedPluginIds = new Set(selectedMentions.flatMap((mention) => mention.kind === "plugin" ? [mention.id] : []));
    const pluginItems: AtChoice[] = plugins
      .filter((plugin) => plugin.installed && plugin.enabled && plugin.available && !selectedPluginIds.has(plugin.id) &&
        (matches(plugin.name) || matches(plugin.display_name) || matches(plugin.description)))
      .sort((a, b) => a.display_name.localeCompare(b.display_name))
      .map((plugin) => ({
        id: `at-plugin.${plugin.id}`,
        type: "plugin",
        title: plugin.display_name,
        description: plugin.description,
        pluginId: plugin.id,
      }));
    if (pluginItems.length > 0) {
      sections.push({ key: "plugins", label: t("composer.pluginsGroup"), items: pluginItems });
    }

    if (!q) {
      sections.push({
        key: "files",
        label: "文件和任务",
        items: [
          {
            id: "add-files",
            type: "add-files",
            title: "输入以搜索文件或任务",
            description: "",
            icon: "paperclip",
          },
        ],
      });
    }
    setAtSections(sections);
    setAtSelected(0);
    if (!q) return;

    const timer = window.setTimeout(() => {
      searchProjectFiles(activeProjectPath, q, 24).then((result) => {
        if (cancelled) return;
        const mentionedPaths = new Set(
          selectedMentions.flatMap((mention) =>
            mention.kind === "file" || mention.kind === "folder" ? [mention.path] : [],
          ),
        );
        const fileItems: AtChoice[] = result.entries
          .filter((entry) => !mentionedPaths.has(entry.path))
          .slice(0, 12)
          .map((entry) => ({
            id: `at-file.${entry.path}`,
            type: "file",
            title: entry.name,
            description: entry.rel_path,
            icon: entry.is_dir ? "folder" : "file",
            entry,
          }));
        if (fileItems.length > 0) {
          setAtSections([...sections, { key: "files", label: "文件和任务", items: fileItems }]);
        }
      })
      .catch(() => {
        // Keep mode and plugin matches available when project search fails.
      });
    }, 80);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [activeAtToken, activeProjectPath, plugins, selectedMentions, slashOpen, t]);

  const chooseSlash = (cmd: SlashChoice) => {
    const replaceWholeRootSlash = !activeSlashToken && draftValue.startsWith("/");
    const rawStart = activeSlashToken
      ? activeSlashToken.start
      : replaceWholeRootSlash
      ? 0
      : cursorPos;
    const rawEnd = activeSlashToken
      ? activeSlashToken.end
      : replaceWholeRootSlash
      ? draftValue.length
      : cursorPos;
    const prefix = draftValue.slice(0, rawStart);
    const suffix = draftValue.slice(rawEnd);

    if (cmd.skillName !== undefined) {
      if (selectedSkills.some((skill) => skill.id === (cmd.skillId ?? cmd.id))) {
        setSlashResults([]);
        setSlashSelected(0);
        requestAnimationFrame(() => textareaRef.current?.focus());
        return;
      }
      const nextValue = `${prefix}${SKILL_MARKER}${suffix}`;
      const insertedSkill = {
        id: cmd.skillId ?? cmd.id,
        name: cmd.skillName,
      };
      const insertAt = countSkillMarkers(prefix);
      setSelectedSkills((prev) => {
        const next = [...prev];
        next.splice(insertAt, 0, insertedSkill);
        return next;
      });
      setDraftValue(nextValue);
      pendingDraftValueRef.current = stripSkillMarkers(nextValue);
      onChange(stripSkillMarkers(nextValue));
      setCursorPos(rawStart + 1);
      setSelectionRange(null);
      focusEditorAt(nextValue, rawStart + 1);
    } else {
      const insertText = cmd.insertText ?? `${cmd.title} `;
      const nextValue = `${prefix}${insertText}${suffix.startsWith(" ") ? suffix.slice(1) : suffix}`;
      setDraftValue(nextValue);
      pendingDraftValueRef.current = stripSkillMarkers(nextValue);
      onChange(stripSkillMarkers(nextValue));
      setCursorPos(rawStart + insertText.length);
      setSelectionRange(null);
      focusEditorAt(nextValue, rawStart + insertText.length);
    }
    setSlashResults([]);
    setSlashSelected(0);
  };

  const insertMentionMarker = (
    rawStart: number,
    rawEnd: number,
    mention: InlineMention,
  ) => {
    const prefix = draftValue.slice(0, rawStart);
    const suffix = draftValue.slice(rawEnd);
    const spacer = suffix && !/^\s/.test(suffix) ? " " : " ";
    const nextValue = `${prefix}${MENTION_MARKER}${spacer}${suffix}`;
    const insertAt = countMentionMarkers(prefix);
    setSelectedMentions((prev) => {
      const next = [...prev];
      next.splice(insertAt, 0, mention);
      return next;
    });
    setDraftValue(nextValue);
    pendingDraftValueRef.current = stripSkillMarkers(nextValue);
    onChange(stripSkillMarkers(nextValue));
    const caret = rawStart + 1 + spacer.length;
    setCursorPos(caret);
    setSelectionRange(null);
    focusEditorAt(nextValue, caret);
  };

  const chooseAt = (choice: AtChoice) => {
    if (!activeAtToken) return;
    const rawStart = activeAtToken.start;
    const rawEnd = activeAtToken.end;

    if (choice.type === "add-files") {
      message.info("继续在 @ 后输入文件名或文件夹名即可搜索");
      setAtSelected(0);
      requestAnimationFrame(() => textareaRef.current?.focus());
      return;
    }

    if (choice.type === "goal" || choice.type === "plan-mode") {
      const nextDraft = draftValue.slice(0, rawStart) + draftValue.slice(rawEnd);
      setDraftMode(choice.type === "goal" ? "goal" : "plan");
      setDraftValue(nextDraft);
      setCursorPos(rawStart);
      setSelectionRange(null);
      const nextText = stripSkillMarkers(nextDraft);
      pendingDraftValueRef.current = nextText;
      onChange(nextText);
      focusEditorAt(nextDraft, rawStart);
    } else if (choice.type === "file") {
      insertMentionMarker(rawStart, rawEnd, {
        kind: choice.entry.is_dir ? "folder" : "file",
        path: choice.entry.path,
        relPath: choice.entry.rel_path,
        name: choice.entry.name,
        isDir: choice.entry.is_dir,
      });
    } else if (choice.type === "plugin") {
      insertMentionMarker(rawStart, rawEnd, {
        kind: "plugin",
        id: choice.pluginId,
        name: choice.title,
      });
    }

    setAtSections([]);
    setAtSelected(0);
  };

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsModelDropdownOpen(false);
      }
      if (approvalDropdownRef.current && !approvalDropdownRef.current.contains(e.target as Node)) {
        setIsApprovalDropdownOpen(false);
      }
    };
    if (isModelDropdownOpen || isApprovalDropdownOpen) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [isModelDropdownOpen, isApprovalDropdownOpen]);

  useEffect(() => {
    const open = isModelDropdownOpen || isApprovalDropdownOpen || isContextPopoverOpen;
    if (open === lastReportedOverlayOpen.current) return;
    lastReportedOverlayOpen.current = open;
    onOverlayOpenChange?.(open);
  }, [isModelDropdownOpen, isApprovalDropdownOpen, isContextPopoverOpen, onOverlayOpenChange]);

  useEffect(() => {
    if (overlayCloseSignal > lastOverlayCloseSignal.current) {
      lastOverlayCloseSignal.current = overlayCloseSignal;
      setIsModelDropdownOpen(false);
      setIsApprovalDropdownOpen(false);
      setIsContextPopoverOpen(false);
    }
  }, [overlayCloseSignal]);

  const draftText = stripSkillMarkers(draftValue).trim();
  const hasComposerContent = mode === "goal"
    ? Boolean(draftText)
    : Boolean(draftText || attachments.length || selectedSkills.length || selectedMentions.length);

  const submitWithAttachments = () => {
    if (!hasComposerContent) return;
    // A standalone slash command must still reach the dispatcher without a mode context prefix.
    // Keep unrecognized slash-prefixed text and drafts with attached context as normal input.
    const isStandaloneSlash = slashTitles.has(draftText.split(/\s+/, 1)[0]) &&
      selectedSkills.length === 0 && selectedMentions.length === 0 && attachments.length === 0;
    if (!selectedModel && !(mode === null ? draftText.startsWith("/") : isStandaloneSlash)) {
      message.warning(t(activeModelOptions.length ? "composer.chooseModelBeforeSend" : "composer.noModels"));
      return;
    }
    const readyAttachments = attachments.filter((item) => item.status === "ready");
    const surface = composerRef.current?.querySelector<HTMLElement>("[data-message-composer-surface]");
    if (textareaRef.current && mirrorRef.current && surface) {
      messageSendMotion?.prepare(textareaRef.current, mirrorRef.current, surface, draftValue);
    }
    const mentions: ComposerMention[] = [...selectedMentions];
    if (!isStandaloneSlash) {
      if (mode === "goal") mentions.push({ kind: "goal", text: draftText });
      if (draftMode === "plan") mentions.push({ kind: "plan_mode" });
    }
    onSubmit(readyAttachments, selectedSkills, mentions, draftValue);
    setDraftMode(null);
    if (readyAttachments.length > 0) setAttachments([]);
    if (selectedSkills.length > 0) setSelectedSkills([]);
    if (selectedMentions.length > 0) setSelectedMentions([]);
    setDraftValue("");
    setCursorPos(0);
    setSelectionRange(null);
    pendingDraftValueRef.current = "";
  };

  const addAttachment = (attachment: ComposerAttachment) => {
    setAttachments((prev) => [...prev, attachment]);
  };

  const showAttachmentLimitWarning = () => {
    message.warning(attachmentLimitMessage);
  };

  const removeAttachment = (id: string) => {
    setAttachments((prev) => prev.filter((item) => item.id !== id));
    void attachmentRemove(id).catch(() => {
      /* The UI removal should not be blocked by cleanup failures. */
    });
  };

  const attachmentId = () =>
    globalThis.crypto?.randomUUID?.() ?? `att_${Date.now()}_${Math.random().toString(16).slice(2)}`;

  const readFileAsDataUrl = (file: File) =>
    new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result ?? ""));
      reader.onerror = () => reject(reader.error ?? new Error(t("composer.attachments.readFailed")));
      reader.readAsDataURL(file);
    });

  const readFileAsText = (file: File) =>
    new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result ?? ""));
      reader.onerror = () => reject(reader.error ?? new Error(t("composer.attachments.readFailed")));
      reader.readAsText(file);
    });

  const addTextAttachment = async (text: string, source: ComposerAttachment["source"], name?: string) => {
    if (attachments.length >= MAX_COMPOSER_ATTACHMENTS) {
      showAttachmentLimitWarning();
      return;
    }
    const id = attachmentId();
    const attachmentName =
      name ?? `pasted-text-${new Date().toISOString().replace(/[:.]/g, "-")}.txt`;
    const size = new Blob([text]).size;
    addAttachment({
      id,
      kind: "text",
      name: attachmentName,
      mime: "text/plain",
      size,
      source,
      status: "processing",
    });
    try {
      const persisted = await attachmentIngest({
        id,
        session_id: null,
        kind: "text",
        name: attachmentName,
        mime: "text/plain",
        source,
        text,
      });
      markAttachmentReady(id, {
        size: persisted.size_bytes || size,
        extractedText: persisted.extracted_text ?? text,
        storageDir: persisted.storage_dir,
        originalPath: persisted.original_path ?? undefined,
        sha256: persisted.sha256 ?? undefined,
        backendMessage: persisted.message ?? undefined,
      });
    } catch (error) {
      markAttachmentError(id, error);
    }
  };

  const isTextLikeFile = (file: File) =>
    file.type.startsWith("text/") ||
    /\.(txt|md|markdown|json|log|yaml|yml|xml|ts|tsx|js|jsx|mjs|cjs|rs|go|py|java|kt|css|scss|html|htm|env|gitignore|npmrc|toml|ini|conf|cfg|sh|bash|zsh|fish|ps1|bat|cmd)$/i.test(
      file.name,
    );

  const markAttachmentReady = (id: string, patch: Partial<ComposerAttachment>) => {
    setAttachments((prev) =>
      prev.map((item) => (item.id === id ? { ...item, ...patch, status: "ready" } : item)),
    );
  };

  const markAttachmentError = (id: string, error: unknown) => {
    setAttachments((prev) =>
      prev.map((item) =>
        item.id === id
          ? { ...item, status: "error", error: error instanceof Error ? error.message : String(error) }
          : item,
      ),
    );
  };

  const addFiles = async (files: FileList | File[], source: ComposerAttachment["source"]) => {
    const incoming = Array.from(files);
    const remaining = MAX_COMPOSER_ATTACHMENTS - attachments.length;
    if (remaining <= 0) {
      showAttachmentLimitWarning();
      return;
    }
    const accepted = incoming.slice(0, remaining);
    if (incoming.length > accepted.length) {
      showAttachmentLimitWarning();
    }

    for (const file of accepted) {
      const id = attachmentId();
      const isImage = file.type.toLowerCase().startsWith("image/");
      const isText = isTextLikeFile(file);
      const base: ComposerAttachment = {
        id,
        kind: isImage ? "image" : isText ? "text" : "file",
        name: file.name || (isImage ? "pasted-image.png" : "attachment"),
        mime: file.type || "application/octet-stream",
        size: file.size,
        source,
        status: "processing",
      };
      addAttachment(base);
      try {
        if (isImage) {
          validateComposerImage(file);
          const dataUrl = await readFileAsDataUrl(file);
          const persisted = await attachmentIngest({
            id,
            session_id: null,
            kind: "image",
            name: base.name,
            mime: base.mime,
            source,
            data_url: dataUrl,
          });
          const imagePath = persisted.original_path ?? undefined;
          markAttachmentReady(id, {
            dataUrl,
            size: persisted.size_bytes || file.size,
            storageDir: persisted.storage_dir,
            originalPath: imagePath,
            extractedText: persisted.extracted_text ?? undefined,
            sha256: persisted.sha256 ?? undefined,
            backendMessage: persisted.message ?? undefined,
          });
        } else if (isText) {
          const extractedText = await readFileAsText(file);
          const persisted = await attachmentIngest({
            id,
            session_id: null,
            kind: "text",
            name: base.name,
            mime: base.mime,
            source,
            text: extractedText,
          });
          markAttachmentReady(id, {
            kind: "text",
            size: persisted.size_bytes || file.size,
            extractedText: persisted.extracted_text ?? extractedText,
            storageDir: persisted.storage_dir,
            originalPath: persisted.original_path ?? undefined,
            sha256: persisted.sha256 ?? undefined,
            backendMessage: persisted.message ?? undefined,
          });
        } else {
          const dataUrl = await readFileAsDataUrl(file);
          const persisted = await attachmentIngest({
            id,
            session_id: null,
            kind: "file",
            name: base.name,
            mime: base.mime,
            source,
            data_url: dataUrl,
          });
          markAttachmentReady(id, {
            size: persisted.size_bytes || file.size,
            extractedText: persisted.extracted_text ?? undefined,
            storageDir: persisted.storage_dir,
            originalPath: persisted.original_path ?? undefined,
            sha256: persisted.sha256 ?? undefined,
            backendMessage: persisted.message ?? undefined,
          });
        }
      } catch (error) {
        markAttachmentError(id, error);
      }
    }
  };

  const onPaste = (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const text = event.clipboardData.getData("text/plain");
    const files = Array.from(event.clipboardData.files);
    if (files.length > 0) {
      event.preventDefault();
      void addFiles(files, "paste");
      return;
    }
    if (text.length > LONG_TEXT_ATTACHMENT_THRESHOLD) {
      event.preventDefault();
      void addTextAttachment(text, "paste");
    }
  };

  const onDrop = (event: React.DragEvent<HTMLDivElement>) => {
    if (event.dataTransfer.files.length === 0) return;
    event.preventDefault();
    void addFiles(event.dataTransfer.files, "drop");
  };

  const syncCursorFromTextarea = () => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    const { offset } = restoreSlashDraft(textarea.value, slashPrefix);
    const start = textarea.selectionStart ?? textarea.value.length;
    const end = textarea.selectionEnd ?? start;
    setCursorPos(offset(start));
    setSelectionRange(normalizeSelectionRange(offset(start), offset(end)));
  };

  const handleEditorChange = (event: React.ChangeEvent<HTMLTextAreaElement>) => {
    const textarea = event.currentTarget;
    const { inputType } = event.nativeEvent as InputEvent;
    const isHistoryEdit = inputType === "historyUndo" || inputType === "historyRedo";
    // Native undo can reinsert the one-character token after it was deleted.
    const prefix = slashPrefix || (isHistoryEdit ? lastCommittedSlashPrefix.current : "");
    const { draft: next, offset } = restoreSlashDraft(textarea.value, prefix);
    const stripped = stripSkillMarkers(next);
    const caret = offset(textarea.selectionStart ?? textarea.value.length);
    const previousDraft = draftValue;
    setDraftValue(next);
    setCursorPos(caret);
    setSelectionRange(
      normalizeSelectionRange(
        caret,
        offset(textarea.selectionEnd ?? textarea.value.length),
      ),
    );
    const previousMarkerCount = countSkillMarkers(previousDraft);
    const nextMarkerCount = countSkillMarkers(next);
    if (nextMarkerCount !== previousMarkerCount) {
      setSelectedSkills((prev) => {
        if (nextMarkerCount <= 0) return [];
        if (nextMarkerCount >= previousMarkerCount) return prev.slice(0, nextMarkerCount);
        const removed = previousMarkerCount - nextMarkerCount;
        const removeAt = Math.min(countSkillMarkers(next.slice(0, caret)), Math.max(prev.length - removed, 0));
        const reconciled = [...prev];
        reconciled.splice(removeAt, removed);
        return reconciled.slice(0, nextMarkerCount);
      });
    }
    const previousMentionCount = countMentionMarkers(previousDraft);
    const nextMentionCount = countMentionMarkers(next);
    if (nextMentionCount !== previousMentionCount) {
      setSelectedMentions((prev) => {
        if (nextMentionCount <= 0) return [];
        if (nextMentionCount >= previousMentionCount) return prev.slice(0, nextMentionCount);
        const removed = previousMentionCount - nextMentionCount;
        const removeAt = Math.min(
          countMentionMarkers(next.slice(0, caret)),
          Math.max(prev.length - removed, 0),
        );
        const reconciled = [...prev];
        reconciled.splice(removeAt, removed);
        return reconciled.slice(0, nextMentionCount);
      });
    }
    pendingDraftValueRef.current = stripped;
    onChange(stripped);
  };

  const editorPlaceholder = mode ? t(`composer.${mode}.placeholder`) : placeholder ?? t("composer.placeholder");
  const mirrorContent = (() => {
    const showMirrorCaret = editorFocused && !selectionRange;
    let caretRendered = false;
    const renderCaretAt = (position: number, key: string) => {
      if (!showMirrorCaret || caretRendered || cursorPos !== position) return null;
      caretRendered = true;
      return <span key={key} className="composer-mirror-caret" aria-hidden="true" />;
    };
    const renderMirrorText = (text: string, startOffset: number, keyPrefix: string) => {
      if (!text) return [];
      if (!selectionRange || !rangesOverlap(selectionRange, startOffset, startOffset + text.length)) {
        const caret = cursorPos >= startOffset && cursorPos <= startOffset + text.length
          ? renderCaretAt(cursorPos, `${keyPrefix}-caret`)
          : null;
        if (!caret) return [<span key={`${keyPrefix}-plain`}>{text}</span>];
        const caretIndex = cursorPos - startOffset;
        return [
          caretIndex > 0 ? <span key={`${keyPrefix}-before-caret`}>{text.slice(0, caretIndex)}</span> : null,
          caret,
          caretIndex < text.length ? <span key={`${keyPrefix}-after-caret`}>{text.slice(caretIndex)}</span> : null,
        ].filter(Boolean);
      }

      const nodes: React.ReactNode[] = [];
      const selectedStart = Math.max(selectionRange.start, startOffset) - startOffset;
      const selectedEnd = Math.min(selectionRange.end, startOffset + text.length) - startOffset;
      if (selectedStart > 0) {
        nodes.push(<span key={`${keyPrefix}-before`}>{text.slice(0, selectedStart)}</span>);
      }
      nodes.push(
        <span key={`${keyPrefix}-selected`} className="rounded-[2px] bg-primary text-white">
          {text.slice(selectedStart, selectedEnd)}
        </span>,
      );
      if (selectedEnd < text.length) {
        nodes.push(<span key={`${keyPrefix}-after`}>{text.slice(selectedEnd)}</span>);
      }
      return nodes;
    };

    if (!draftValue) {
      return <span className="text-gray-400">{editorPlaceholder}</span>;
    }
    const markerRegex = /[\uE000\uE001]/g;
    let offset = 0;
    let skillIndex = 0;
    let mentionIndex = 0;
    const nodes: React.ReactNode[] = [];
    let match: RegExpExecArray | null;
    let segmentIndex = 0;
    if (committedSlash) {
      const selected = rangesOverlap(selectionRange, 0, slashPrefix.length);
      const caretBefore = renderCaretAt(0, "slash-before-caret");
      if (caretBefore) nodes.push(caretBefore);
      nodes.push(
        <span key="slash" ref={slashChipRef} className="inline-flex max-w-[min(15rem,50%)] pr-1.5 align-baseline">
          <span data-composer-slash-token className={cn(
            "inline-flex h-5 min-w-0 items-center gap-1 rounded-sm px-1 text-[13px] font-medium leading-none",
            selected ? "bg-primary text-white" : "bg-primary/10 text-primary",
          )}>
            {committedSlash.kind === "skill" && <Layers aria-hidden="true" className="size-3 shrink-0" />}
            {committedSlash.kind === "plugin" && <Puzzle aria-hidden="true" className="size-3 shrink-0" />}
            <span className="truncate">{committedSlash.label}</span>
          </span>
        </span>,
      );
      offset = slashPrefix.length;
      markerRegex.lastIndex = offset;
      const caretAfter = renderCaretAt(offset, "slash-after-caret");
      if (caretAfter) nodes.push(caretAfter);
    }
    while ((match = markerRegex.exec(draftValue)) !== null) {
      const markerStart = match.index;
      const text = draftValue.slice(offset, markerStart);
      if (text) nodes.push(...renderMirrorText(text, offset, `text-${segmentIndex}`));
      offset = markerStart;
      const marker = match[0];
      const markerSelected = rangesOverlap(selectionRange, offset, offset + 1);
      const caretBefore = renderCaretAt(offset, `marker-${segmentIndex}-before-caret`);
      if (caretBefore) nodes.push(caretBefore);

      if (marker === SKILL_MARKER) {
        const skill = selectedSkills[skillIndex];
        if (skill) {
          const skillLabel = skill.name || skill.id;
          nodes.push(
            <HoverInfo key={`skill-${segmentIndex}`} content={`${skill.name} (${skill.id})`}><span
              className={`mx-0.5 inline-flex h-[20px] max-w-[180px] items-center rounded-[3px] px-1 text-[13px] font-medium leading-none align-baseline ${
                markerSelected
                  ? "bg-primary text-white"
                  : "border border-primary/20 bg-primary/10 text-primary"
              }`}

            >
              <Layers aria-hidden="true" className="mr-1 size-3 shrink-0" />
              <span className="truncate">{skillLabel}</span>
            </span></HoverInfo>,
          );
        }
        skillIndex += 1;
      } else {
        const mention = selectedMentions[mentionIndex];
        if (mention) {
          const label = mention.kind === "plugin" ? mention.name : mention.relPath || mention.name;
          const title = mention.kind === "plugin" ? mention.id : mention.path;
          nodes.push(
            <HoverInfo key={`mention-${segmentIndex}`} content={title}><span
              className={`mx-0.5 inline-flex h-[20px] max-w-[220px] items-center rounded-[3px] px-1 text-[13px] font-medium leading-none align-baseline ${
                markerSelected
                  ? "bg-primary text-white"
                  : "bg-black/5 text-text-base"
              }`}

            >
              {mention.kind === "plugin"
                ? <Puzzle aria-hidden="true" className="mr-1 size-3 text-text-secondary" />
                : <FontAwesomeIcon icon={["fas", mention.kind === "folder" ? "folder" : "file"]} className="mr-1 text-[11px] text-text-secondary" />}
              <span className="truncate">{label}</span>
            </span></HoverInfo>,
          );
        }
        mentionIndex += 1;
      }

      offset += 1;
      const caretAfter = renderCaretAt(offset, `marker-${segmentIndex}-after-caret`);
      if (caretAfter) nodes.push(caretAfter);
      segmentIndex += 1;
    }
    const tail = draftValue.slice(offset);
    if (tail) nodes.push(...renderMirrorText(tail, offset, `text-${segmentIndex}`));
    return nodes;
  })();

  const handleCopy = (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const textarea = event.currentTarget;
    const { draft, offset } = restoreSlashDraft(textarea.value, slashPrefix);
    const start = offset(textarea.selectionStart);
    const end = offset(textarea.selectionEnd);
    if (start === end) return;
    const selectedText = stripSkillMarkers(draft.slice(start, end));
    event.preventDefault();
    event.clipboardData.setData("text/plain", selectedText);
  };

  const handleCut = (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const textarea = event.currentTarget;
    const { draft, offset } = restoreSlashDraft(textarea.value, slashPrefix);
    const start = offset(textarea.selectionStart);
    const end = offset(textarea.selectionEnd);
    if (start === end) return;
    const selectedText = stripSkillMarkers(draft.slice(start, end));
    const nextValue = `${draft.slice(0, start)}${draft.slice(end)}`;
    const stripped = stripSkillMarkers(nextValue);
    event.preventDefault();
    event.clipboardData.setData("text/plain", selectedText);
    setDraftValue(nextValue);
    setCursorPos(start);
    setSelectionRange(null);
    const removedMarkers = countSkillMarkers(draft.slice(start, end));
    if (removedMarkers > 0) {
      const removeAt = countSkillMarkers(draft.slice(0, start));
      setSelectedSkills((prev) => {
        const next = [...prev];
        next.splice(removeAt, removedMarkers);
        return next;
      });
    }
    const removedMentionMarkers = countMentionMarkers(draft.slice(start, end));
    if (removedMentionMarkers > 0) {
      const removeAt = countMentionMarkers(draft.slice(0, start));
      setSelectedMentions((prev) => {
        const next = [...prev];
        next.splice(removeAt, removedMentionMarkers);
        return next;
      });
    }
    pendingDraftValueRef.current = stripped;
    onChange(stripped);
    focusEditorAt(nextValue, start);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return;
    if (atOpen && e.key === "ArrowDown") {
      e.preventDefault();
      setSuggestionNavigation(value => value + 1);
      setAtSelected((s) => Math.min(s + 1, atEntries.length - 1));
      return;
    }
    if (atOpen && e.key === "ArrowUp") {
      e.preventDefault();
      setSuggestionNavigation(value => value + 1);
      setAtSelected((s) => Math.max(s - 1, 0));
      return;
    }
    if (atOpen && (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey))) {
      const selected = atEntries[atSelected];
      if (selected) {
        e.preventDefault();
        chooseAt(selected);
        return;
      }
    }
    if (atOpen && e.key === "Escape") {
      e.preventDefault();
      setAtSections([]);
      return;
    }
    if (slashOpen && e.key === "ArrowDown") {
      e.preventDefault();
      setSuggestionNavigation(value => value + 1);
      setSlashSelected((s) => Math.min(s + 1, slashEntries.length - 1));
      return;
    }
    if (slashOpen && e.key === "ArrowUp") {
      e.preventDefault();
      setSuggestionNavigation(value => value + 1);
      setSlashSelected((s) => Math.max(s - 1, 0));
      return;
    }
    if (slashOpen && (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey))) {
      const selected = slashEntries[slashSelected];
      const exact = selected?.skillName ? false : selected?.title === stripSkillMarkers(draftValue).trim();
      if (selected && (!exact || e.key === "Tab")) {
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
    if (draftMode && e.key === "Escape") {
      e.preventDefault();
      setDraftMode(null);
      return;
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (!busy) submitWithAttachments();
    }
  };

  return (
    <div ref={composerRef} className="group/composer relative w-full">
      <InputSurface
        data-message-composer-surface
        className="flex flex-col overflow-visible rounded-[24px] shadow-[0_8px_24px_rgba(31,38,48,0.09)] focus-within:shadow-[0_10px_30px_rgba(31,38,48,0.13)]"
        onDragOver={(event) => event.preventDefault()}
        onDrop={onDrop}
      >
        <div className="relative flex w-full flex-col p-3.5 pb-2.5">
      <ComposerSuggestPanel
        id={`${suggestId}-at`}
        open={atOpen}
        sections={atSections}
        selectedIndex={atSelected}
        navigationKey={suggestionNavigation}
        getKey={(item) => item.id}
        onSelect={chooseAt}
        onHover={setAtSelected}
        renderItem={(item, selected) => (
          <div
            className={`grid w-full grid-cols-[18px_minmax(0,1fr)] items-center gap-2.5 px-2 py-2 text-left text-[13px] transition-colors duration-150 ${
              selected ? "font-medium text-text-base" : "text-text-secondary"
            }`}
          >
            <span className="flex h-4 w-4 items-center justify-center text-text-base">
              {item.type === "plugin" ? <Puzzle aria-hidden="true" className="size-4" />
                : item.type === "goal" ? <GoalTarget aria-hidden="true" className="size-4" />
                : item.type === "plan-mode" ? <ListChecks aria-hidden="true" className="size-4" />
                : <FontAwesomeIcon icon={["fas", item.icon]} className="text-[12px]" />}
            </span>
            <span className="min-w-0">
              <span className="block truncate font-medium text-text-base">{item.title}</span>
              {item.description && (
                <span className="block truncate text-[12px] text-text-secondary">
                  {item.description}
                </span>
              )}
            </span>
          </div>
        )}
      />
      <ComposerSuggestPanel
        id={`${suggestId}-slash`}
        open={slashOpen}
        sections={slashSections}
        selectedIndex={slashSelected}
        navigationKey={suggestionNavigation}
        getKey={(cmd) => cmd.id}
        onSelect={chooseSlash}
        onHover={setSlashSelected}
        renderTooltip={(cmd) => (
          <>
            <div className="font-medium">{cmd.title}</div>
            {cmd.description && (
              <div className="mt-1 max-w-sm text-bg-base/80">{cmd.description}</div>
            )}
          </>
        )}
        renderItem={(cmd, selected) => (
          <div
            className={`composer-slash-row grid w-full grid-cols-[130px_minmax(0,1fr)_64px] items-center gap-2.5 px-2 py-2 text-left text-[13px] transition-colors duration-150 ${
              selected ? "font-medium text-text-base" : "text-text-secondary"
            }`}
          >
            <span className="flex min-w-0 items-center gap-1.5 font-medium text-text-base">
              {(cmd.skillId || slashCommandKind(cmd.title) === "skill") && (
                <Layers aria-hidden="true" className="size-3 shrink-0" />
              )}
              <span className="truncate">{cmd.title}</span>
            </span>
            <span className="truncate text-[12px] leading-snug text-text-secondary">
              {cmd.description}
            </span>
            <span className="composer-slash-category truncate text-right text-[11px] text-text-secondary">
              {cmd.category}
            </span>
          </div>
        )}
      />
      <ComposerAttachmentList attachments={attachments} onRemove={removeAttachment} />

      <div className="relative w-full">
        <div
          ref={mirrorRef}
          data-message-composer-content
          aria-hidden="true"
          className="pointer-events-none absolute inset-x-0 top-0 overflow-hidden custom-scrollbar whitespace-pre-wrap break-words bg-transparent text-sm leading-6 text-text-base"
          style={{
            minHeight: `${COMPOSER_TEXTAREA_MIN_HEIGHT}px`,
            maxHeight: `${textareaMaxHeight}px`,
          }}
        >
          {mirrorContent}
        </div>
        <textarea
          ref={textareaRef}
          aria-label={committedSlash ? `${editorPlaceholder} (${committedSlash.prefix.trimEnd()})` : editorPlaceholder}
          aria-autocomplete="list"
          aria-controls={atOpen ? `${suggestId}-at` : slashOpen ? `${suggestId}-slash` : undefined}
          aria-activedescendant={atOpen ? `${suggestId}-at-option-${atSelected}` : slashOpen ? `${suggestId}-slash-option-${slashSelected}` : undefined}
          className={`composer-textarea custom-scrollbar relative z-10 block w-full resize-none bg-transparent text-sm leading-6 text-transparent outline-none placeholder-transparent ${
            draftValue ? "" : "composer-textarea--empty"
          }`}
          style={{
            minHeight: `${COMPOSER_TEXTAREA_MIN_HEIGHT}px`,
            maxHeight: `${textareaMaxHeight}px`,
          }}
          placeholder={editorPlaceholder}
          value={editorValue}
          onChange={handleEditorChange}
          onPaste={onPaste}
          onCopy={handleCopy}
          onCut={handleCut}
          onKeyDown={onKeyDown}
          onSelect={syncCursorFromTextarea}
          onClick={syncCursorFromTextarea}
          onKeyUp={syncCursorFromTextarea}
          onFocus={() => {
            setEditorFocused(true);
            syncCursorFromTextarea();
          }}
          onBlur={() => {
            setEditorFocused(false);
            setSelectionRange(null);
          }}
          onScroll={(event) => {
            if (!mirrorRef.current) return;
            mirrorRef.current.scrollTop = event.currentTarget.scrollTop;
            mirrorRef.current.scrollLeft = event.currentTarget.scrollLeft;
          }}
        />
      </div>

      <div className="composer-toolbar mt-2.5 flex h-8 items-center gap-3">
        <div className="composer-primary-tools flex min-w-0 flex-1 items-center gap-1.5 overflow-visible">
          <input
            ref={fileInputRef}
            type="file"
            multiple
            className="hidden"
            onChange={(event) => {
              if (event.target.files) void addFiles(event.target.files, "picker");
              event.currentTarget.value = "";
            }}
          />
          <ComposerAttachButton
            disabled={attachments.length >= MAX_COMPOSER_ATTACHMENTS}
            disabledTitle={attachmentLimitMessage}
            onClick={() => fileInputRef.current?.click()}
          />

          {visibleOptions.length > 0 && (
            <div ref={approvalDropdownRef} className="min-w-0 max-w-full">
              <MorphingMenuShell
                open={isApprovalDropdownOpen}
                onOpenChange={(next) => {
                  if (next) {
                    setIsModelDropdownOpen(false);
                    setIsContextPopoverOpen(false);
                  }
                  setIsApprovalDropdownOpen(next);
                }}
                layoutId={approvalMorphLayoutId}
                trigger={
                  <button
                    type="button"
                    className={cn(
                      "flex h-8 max-w-full min-w-0 flex-shrink items-center whitespace-nowrap rounded-full px-3 text-[11px] font-medium",
                      MOTION.fast,
                      selectedApproval?.id === "full"
                        ? cn(
                            "text-[#e25507] bg-[#e25507]/10 hover:bg-[#e25507]/18 active:bg-[#e25507]/22",
                            isApprovalDropdownOpen && "bg-[#e25507]/18",
                          )
                        : cn(
                            "text-text-secondary bg-black/5 hover:bg-black/10 hover:text-text-base active:bg-black/[0.12]",
                            isApprovalDropdownOpen && "bg-black/10 text-text-base",
                          ),
                    )}
                    onClick={() => {
                      const next = !isApprovalDropdownOpen;
                      if (next) {
                        setIsModelDropdownOpen(false);
                        setIsContextPopoverOpen(false);
                      }
                      setIsApprovalDropdownOpen(next);
                    }}
                  >
                    {selectedApproval && (
                      <>
                        <selectedApproval.icon
                          size={12}
                          className={cn(
                            "mr-1.5 shrink-0",
                            selectedApproval.id === "full" ? "text-[#e25507]" : "text-text-secondary",
                          )}
                        />
                        <span className="truncate">{t(selectedApproval.label)}</span>
                      </>
                    )}
                    <ChevronDown size={10} className="ml-1 shrink-0" />
                  </button>
                }
                className="min-w-0 max-w-full"
                panelClassName="w-[min(288px,calc(100vw-48px))]"
                panelShadow="0 4px 12px rgba(0,0,0,0.08)"
                panelAlign="left"
              >
                <div className="p-1.5">
                  <SlidingMenuList
                    activeId={selectedApproval?.id ?? ""}
                    pillClassName={cn(
                      "left-0 right-0 rounded-xl bg-[#f5f5f5]",
                      selectedApproval?.id === "full" && "bg-[#e25507]/10",
                    )}
                    className="flex w-full flex-col gap-0.5"
                  >
                    {visibleOptions.map((opt) => {
                      const selected = selectedApproval?.id === opt.id;
                      const isFull = opt.id === "full";
                      return (
                        <div
                          key={opt.id}
                          {...{ [MENU_ITEM_ATTR]: opt.id }}
                          className={cn(
                            "relative z-[1] flex w-full cursor-pointer items-start gap-[7px] rounded-xl px-2 py-2 text-left",
                            MOTION.fast,
                            selected
                              ? isFull
                                ? "font-medium text-[#e25507]"
                                : "font-medium text-text-base"
                              : "text-text-base",
                          )}
                          onClick={() => chooseApproval(opt)}
                        >
                          <div className="mt-0.5 flex w-3 shrink-0 justify-center">
                            <opt.icon
                              size={12}
                              className={cn(
                                selected && isFull ? "text-[#e25507]" : "text-text-secondary",
                              )}
                            />
                          </div>
                          <div className="min-w-0 flex-1">
                            <div className={cn("pr-3.5 text-[12px] font-medium leading-[17px]", selected && !isFull && "text-text-base")}>
                              {t(opt.label)}
                            </div>
                            <div
                              className={cn(
                                "mt-0.5 text-[11px] leading-[15px]",
                                selected && isFull ? "text-[#e25507]/80" : "text-[#8c8c8c]",
                              )}
                            >
                              {t(opt.desc)}
                            </div>
                          </div>
                          {selected && (
                            <Check
                              size={10}
                              className={cn("absolute right-2 top-[11px] shrink-0", isFull ? "text-[#e25507]" : "text-text-secondary")}
                            />
                          )}
                        </div>
                      );
                    })}
                  </SlidingMenuList>
                </div>
              </MorphingMenuShell>
            </div>
          )}
          {mode && (
            <HoverInfo content={t(`composer.${mode}.exitHint`)}>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t(`composer.${mode}.title`)}
                aria-pressed={true}
                className="size-8 rounded-full bg-primary/10 text-primary hover:bg-primary/15 hover:text-primary"
                onClick={() => {
                  if (mode === "plan" && planMode === true && !busy) onExitPlanMode?.();
                  setDraftMode(null);
                  textareaRef.current?.focus();
                }}
              >
                {mode === "goal" ? <GoalTarget aria-hidden="true" className="size-4" />
                  : <ListChecks aria-hidden="true" className="size-4" />}
              </Button>
            </HoverInfo>
          )}
        </div>

        <div className="composer-model-tools flex h-8 shrink-0 items-center gap-2">
          <ContextCapacityIndicator
            snapshot={contextUsage}
            contextWindow={
              modelOptions.find((model) => model.id === selectedModel)?.contextWindow
            }
            fallbackPromptTokens={contextUsageFallbackTokens}
            popoverSuppressed={isModelDropdownOpen || isApprovalDropdownOpen}
            overlayCloseSignal={overlayCloseSignal}
            onPopoverOpenChange={(next) => {
              if (next) {
                setIsModelDropdownOpen(false);
                setIsApprovalDropdownOpen(false);
              }
              setIsContextPopoverOpen(next);
            }}
          />
          <div className="relative flex h-8 items-center" ref={dropdownRef}>
            <ModelThinkingSelector
              open={isModelDropdownOpen}
              disabled={busy && !onStop}
              switching={switching}
              models={modelOptions}
              thinkingLevels={modelOptions.find((model) => model.id === selectedModel)?.thinkingLevels}
              thinkingDisable={modelOptions.find((model) => model.id === selectedModel)?.thinkingDisable}
              selectedModel={selectedModel}
              selectedThinking={selectedThinking}
              thinkingOptions={THINKING_OPTIONS.map((opt) => ({
                ...opt,
                label: t(opt.label),
              }))}
              selectModelLabel={t("composer.selectModel")}
              noModelsLabel={t("composer.noModels")}
              onOpenChange={(next) => {
                if (next) {
                  setIsApprovalDropdownOpen(false);
                  setIsContextPopoverOpen(false);
                }
                setIsModelDropdownOpen(next);
              }}
              onChooseModel={chooseModel}
              onChooseThinking={chooseThinking}
            />
          </div>
          <HoverInfo content={busy ? t(stopping ? "composer.stopping" : "composer.stop") : undefined}><button
            onClick={() => {
              if (busy) {
                onStop?.();
              } else {
                submitWithAttachments();
              }
            }}
            disabled={busy ? !onStop || stopping : !hasComposerContent}
            aria-label={busy ? t(stopping ? "composer.stopping" : "composer.stop") : t("composer.send")}

            className={cn(
              "composer-send-btn",
              busy
                ? onStop
                  ? "composer-send-btn--stop"
                  : "composer-send-btn--idle"
                : hasComposerContent
                  ? mode === "plan"
                    ? "composer-send-btn--plan"
                    : "composer-send-btn--ready"
                  : "composer-send-btn--idle",
            )}
          >
            {stopping
              ? <Loader2 className="h-3.5 w-3.5 animate-spin" />
              : <FontAwesomeIcon icon={busy ? ["fas", "stop"] : ["fas", "arrow-up"]} className="text-[11px]" />}
          </button></HoverInfo>
        </div>
      </div>
        </div>
      </InputSurface>

      {footer && (
        <div
          className={cn(
            "relative mt-2 flex min-h-[48px] w-full items-center overflow-visible rounded-[20px] bg-gradient-to-r from-hover-bg via-selection-bg/70 to-primary/10 px-3.5 py-2 shadow-[0_6px_20px_rgba(0,0,0,0.06)] group-focus-within/composer:shadow-[0_8px_24px_rgba(0,0,0,0.10)]",
            MOTION.standard,
          )}
        >
          {footer}
        </div>
      )}
    </div>
  );
}



