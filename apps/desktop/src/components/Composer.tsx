import { useState, useRef, useEffect, useCallback, useLayoutEffect, useMemo } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import {
  SETTINGS_CHANGED_EVENT,
  attachmentIngest,
  attachmentRemove,
  getActivePermissionPreset,
  getCommands,
  getPermissionPresetVisibility,
  getSettings,
  listSkills,
  setActivePermissionPreset,
  setChatModel,
  setThinkingDepth,
} from "../api";
import { message } from "./message";
import type { Command, ComposerAttachment, ComposerSkillSelection, Skill } from "../types";

const LONG_TEXT_ATTACHMENT_THRESHOLD = 8 * 1024;
const MAX_COMPOSER_ATTACHMENTS = 5;
const COMPOSER_TEXTAREA_MIN_HEIGHT = 60;
const COMPOSER_TEXTAREA_DEFAULT_MAX_HEIGHT = 300;
const SKILL_MARKER = "\uE000";

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
type SelectionRange = {
  start: number;
  end: number;
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

/** Split a model id into a two-tier {name, version} label for display.
 * e.g. "deepseek-v4-flash" -> {name:"deepseek", version:"v4-flash"}. Ids without a
 * hyphen render as a single name. */
function labelFor(id: string): { name: string; version: string } {
  const dash = id.indexOf("-");
  if (dash <= 0) return { name: id, version: "" };
  return { name: id.slice(0, dash), version: id.slice(dash + 1) };
}

function stripSkillMarkers(value: string): string {
  return value.replace(/\uE000/g, "");
}

function countVisibleChars(value: string): number {
  return stripSkillMarkers(value).length;
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
  const match = beforeCursor.match(/(^|[\s\uE000])\/([a-zA-Z0-9_:-]*)$/);
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

interface Props {
  value: string;
  onChange: (v: string) => void;
  onSubmit: (
    attachments?: ComposerAttachment[],
    selectedSkill?: ComposerSkillSelection | null,
  ) => void;
  placeholder?: string;
  /** True while a run is streaming: disables submit and shows a busy button. */
  busy?: boolean;
  /** Stop the in-flight run (turns the busy button into a stop button). */
  onStop?: () => void;
  /** True when the current session is in read-only Plan mode. */
  planMode?: boolean;
  /** Optional footer content rendered seamlessly at the bottom of the composer. */
  footer?: React.ReactNode;
  /** Maximum auto-expanded textarea height in pixels. */
  textareaMaxHeight?: number;
}

export function Composer({
  value,
  onChange,
  onSubmit,
  placeholder,
  busy = false,
  onStop,
  planMode = false,
  footer,
  textareaMaxHeight = COMPOSER_TEXTAREA_DEFAULT_MAX_HEIGHT,
}: Props) {
  const { t } = useTranslation();
  const [isModelDropdownOpen, setIsModelDropdownOpen] = useState(false);
  const [isThinkingDropdownOpen, setIsThinkingDropdownOpen] = useState(false);
  const [isApprovalDropdownOpen, setIsApprovalDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const thinkingDropdownRef = useRef<HTMLDivElement>(null);
  const approvalDropdownRef = useRef<HTMLDivElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const mirrorRef = useRef<HTMLDivElement>(null);
  const [attachments, setAttachments] = useState<ComposerAttachment[]>([]);
  // Real discovered models + the active chat model, loaded from the backend
  // settings (populated by API-key validation at login).
  const [models, setModels] = useState<string[]>([]);
  const [selectedModel, setSelectedModel] = useState<string>("");
  const [selectedThinking, setSelectedThinking] = useState<"simple" | "medium" | "deep">("medium");
  const [switching, setSwitching] = useState(false);
  const [slashResults, setSlashResults] = useState<SlashChoice[]>([]);
  const [slashSelected, setSlashSelected] = useState(0);
  const [selectedSkill, setSelectedSkill] = useState<ComposerSkillSelection | null>(null);
  const [draftValue, setDraftValue] = useState(value);
  const [cursorPos, setCursorPos] = useState(value.length);
  const [selectionRange, setSelectionRange] = useState<SelectionRange | null>(null);
  const [editorFocused, setEditorFocused] = useState(false);
  const pendingDraftValueRef = useRef(value);
  // Skill registry mirror, used to populate slash candidates (Channel C v1).
  // Reloads on `SETTINGS_CHANGED_EVENT` so install/uninstall mid-session is
  // reflected without a chat-page refresh.
  const [skills, setSkills] = useState<Skill[]>([]);

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
    const onSettingsChanged = () => load();
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
    setSelectedSkill(null);
    pendingDraftValueRef.current = value;
  }, [draftValue, value]);

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

  // The active model's two-tier label (empty until settings load).
  const selectedLabel = labelFor(selectedModel);

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

  const activeSlashToken = useMemo(
    () => findActiveSlashToken(draftValue, cursorPos),
    [cursorPos, draftValue],
  );

  const slashOpen = Boolean((draftValue.startsWith("/") || activeSlashToken) && slashEntries.length > 0);

  const resizeTextarea = useCallback(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;

    textarea.style.height = "auto";
    const nextHeight = Math.min(
      Math.max(textarea.scrollHeight, COMPOSER_TEXTAREA_MIN_HEIGHT),
      textareaMaxHeight,
    );
    textarea.style.height = `${nextHeight}px`;
    textarea.style.overflowY = textarea.scrollHeight > textareaMaxHeight ? "auto" : "hidden";
  }, [textareaMaxHeight]);

  useLayoutEffect(() => {
    resizeTextarea();
  }, [draftValue, resizeTextarea]);

  useEffect(() => {
    if (draftValue.startsWith("/")) {
      const body = draftValue.slice(1);
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
          { id: "deep", title: "deep", description: "深度思考，给复杂任务更多推理预留", category: "思考" },
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
    }

    if (!activeSlashToken) {
      setSlashResults([]);
      setSlashSelected(0);
      return;
    }

    let cancelled = false;
    const q = activeSlashToken.query.trim().toLowerCase();
    const skillChoices: SlashChoice[] = skills
      .filter(
        (sk) =>
          !q ||
          sk.id.toLowerCase().includes(q) ||
          sk.name.toLowerCase().includes(q),
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
    getCommands("")
      .then((commands) => {
        if (cancelled) return;
        const slash = commands.filter((c) => {
          if (!c.id.startsWith("slash.")) return false;
          const name = c.title.replace(/^\//, "").toLowerCase();
          return !q || name.includes(q);
        });
        setSlashResults([...slash, ...skillChoices]);
        setSlashSelected(0);
      })
      .catch(() => {
        if (!cancelled) {
          setSlashResults(skillChoices);
          setSlashSelected(0);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [activeSlashToken, draftValue, models, selectedModel, skills]);

  const chooseSlash = (cmd: SlashChoice) => {
    const source = stripSkillMarkers(draftValue);
    const replaceWholeRootSlash = !activeSlashToken && draftValue.startsWith("/");
    const visibleStart = activeSlashToken
      ? countVisibleChars(draftValue.slice(0, activeSlashToken.start))
      : replaceWholeRootSlash
      ? 0
      : cursorPos;
    const visibleEnd = activeSlashToken
      ? countVisibleChars(draftValue.slice(0, activeSlashToken.end))
      : replaceWholeRootSlash
      ? source.length
      : cursorPos;
    const prefix = source.slice(0, visibleStart);
    const suffix = source.slice(visibleEnd);

    if (cmd.skillName !== undefined) {
      const nextValue = `${prefix}${SKILL_MARKER}${suffix}`;
      setSelectedSkill({
        id: cmd.skillId ?? cmd.id,
        name: cmd.skillName,
      });
      setDraftValue(nextValue);
      pendingDraftValueRef.current = stripSkillMarkers(nextValue);
      onChange(stripSkillMarkers(nextValue));
      setCursorPos(visibleStart + 1);
      setSelectionRange(null);
      requestAnimationFrame(() => {
        textareaRef.current?.setSelectionRange(visibleStart + 1, visibleStart + 1);
        textareaRef.current?.focus();
      });
    } else {
      const insertText = cmd.insertText ?? `${cmd.title} `;
      const nextValue = `${prefix}${insertText}${suffix}`;
      setSelectedSkill(null);
      setDraftValue(nextValue);
      pendingDraftValueRef.current = stripSkillMarkers(nextValue);
      onChange(stripSkillMarkers(nextValue));
      setCursorPos(visibleStart + insertText.length);
      setSelectionRange(null);
      requestAnimationFrame(() => {
        const caret = visibleStart + insertText.length;
        textareaRef.current?.setSelectionRange(caret, caret);
        textareaRef.current?.focus();
      });
    }
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

  const submitWithAttachments = () => {
    const readyAttachments = attachments.filter((item) => item.status === "ready");
    onSubmit(readyAttachments, selectedSkill);
    if (readyAttachments.length > 0) setAttachments([]);
    if (selectedSkill) setSelectedSkill(null);
    setDraftValue("");
    setCursorPos(0);
    setSelectionRange(null);
    pendingDraftValueRef.current = "";
  };

  const addAttachment = (attachment: ComposerAttachment) => {
    setAttachments((prev) => [...prev, attachment]);
  };

  const showAttachmentLimitWarning = () => {
    message.warning(`Maximum ${MAX_COMPOSER_ATTACHMENTS} attachments allowed`);
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
      reader.onerror = () => reject(reader.error ?? new Error("read file failed"));
      reader.readAsDataURL(file);
    });

  const readFileAsText = (file: File) =>
    new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result ?? ""));
      reader.onerror = () => reject(reader.error ?? new Error("read file failed"));
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
    /\.(txt|md|markdown|json|log|yaml|yml|toml|xml|ts|tsx|js|jsx|mjs|cjs|rs|go|py|java|kt|css|scss|html|htm|env|gitignore|npmrc|toml|ini|conf|cfg|sh|bash|zsh|fish|ps1|bat|cmd)$/i.test(
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
      const isImage = file.type.startsWith("image/");
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
    const start = textarea.selectionStart ?? textarea.value.length;
    const end = textarea.selectionEnd ?? start;
    setCursorPos(start);
    setSelectionRange(normalizeSelectionRange(start, end));
  };

  const handleEditorChange = (event: React.ChangeEvent<HTMLTextAreaElement>) => {
    const next = event.currentTarget.value;
    const stripped = stripSkillMarkers(next);
    setDraftValue(next);
    setCursorPos(event.currentTarget.selectionStart ?? next.length);
    setSelectionRange(
      normalizeSelectionRange(
        event.currentTarget.selectionStart ?? next.length,
        event.currentTarget.selectionEnd ?? event.currentTarget.selectionStart ?? next.length,
      ),
    );
    if (selectedSkill && !next.includes(SKILL_MARKER)) {
      setSelectedSkill(null);
    }
    pendingDraftValueRef.current = stripped;
    onChange(stripped);
  };

  const editorPlaceholder = placeholder ?? t("composer.placeholder");
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
      return (
        <>
          {renderCaretAt(0, "empty-caret")}
          <span className="text-gray-400">{editorPlaceholder}</span>
        </>
      );
    }
    const parts = draftValue.split(SKILL_MARKER);
    let offset = 0;
    return parts.flatMap((part, index) => {
      const nodes: React.ReactNode[] = [];
      const textStart = offset;
      if (part) nodes.push(...renderMirrorText(part, textStart, `text-${index}`));
      offset += part.length;
      if (index < parts.length - 1) {
        const caretBeforeSkill = renderCaretAt(offset, `skill-${index}-before-caret`);
        if (caretBeforeSkill) nodes.push(caretBeforeSkill);
        if (selectedSkill) {
          const skillSelected = rangesOverlap(selectionRange, offset, offset + 1);
          const skillLabel = selectedSkill.name || selectedSkill.id;
          nodes.push(
            <span
              key={`skill-${index}`}
              className={`mx-0.5 inline-flex h-[20px] max-w-[180px] items-center rounded-[3px] px-1 text-[13px] font-medium leading-none align-baseline ${
                skillSelected
                  ? "bg-primary text-white"
                  : "border border-primary/20 bg-primary/10 text-primary"
              }`}
              title={`${selectedSkill.name} (${selectedSkill.id})`}
            >
              <FontAwesomeIcon icon={["fas", "cube"]} className="mr-1 text-[11px]" />
              <span className="truncate">{skillLabel}</span>
            </span>,
          );
        }
        offset += 1;
        const caretAfterSkill = renderCaretAt(offset, `skill-${index}-after-caret`);
        if (caretAfterSkill) nodes.push(caretAfterSkill);
      }
      return nodes;
    });
  })();

  const handleCopy = (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const textarea = event.currentTarget;
    const start = textarea.selectionStart ?? 0;
    const end = textarea.selectionEnd ?? start;
    if (start === end) return;
    const selectedText = stripSkillMarkers(textarea.value.slice(start, end));
    event.preventDefault();
    event.clipboardData.setData("text/plain", selectedText);
  };

  const handleCut = (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const textarea = event.currentTarget;
    const start = textarea.selectionStart ?? 0;
    const end = textarea.selectionEnd ?? start;
    if (start === end) return;
    const selectedText = stripSkillMarkers(textarea.value.slice(start, end));
    const nextValue = `${textarea.value.slice(0, start)}${textarea.value.slice(end)}`;
    const stripped = stripSkillMarkers(nextValue);
    event.preventDefault();
    event.clipboardData.setData("text/plain", selectedText);
    setDraftValue(nextValue);
    setCursorPos(start);
    setSelectionRange(null);
    if (selectedSkill && !nextValue.includes(SKILL_MARKER)) {
      setSelectedSkill(null);
    }
    pendingDraftValueRef.current = stripped;
    onChange(stripped);
    requestAnimationFrame(() => {
      textareaRef.current?.setSelectionRange(start, start);
      textareaRef.current?.focus();
    });
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (slashOpen && e.key === "ArrowDown") {
      e.preventDefault();
      setSlashSelected((s) => Math.min(s + 1, slashEntries.length - 1));
      return;
    }
    if (slashOpen && e.key === "ArrowUp") {
      e.preventDefault();
      setSlashSelected((s) => Math.max(s - 1, 0));
      return;
    }
    if (slashOpen && (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey))) {
      const selected = slashEntries[slashSelected];
      const exact = selected?.skillName ? false : selected?.title === stripSkillMarkers(draftValue).trim();
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
      if (!busy) submitWithAttachments();
    }
  };

  return (
    <div
      className="relative w-full border border-border-theme rounded-2xl shadow-[0_2px_10px_rgba(0,0,0,0.02)] bg-white flex flex-col transition-all focus-within:border-gray-300 focus-within:shadow-md"
      onDragOver={(event) => event.preventDefault()}
      onDrop={onDrop}
    >
      <div className="p-3 pb-2 flex flex-col relative w-full">
      {slashOpen && (
        <div className="absolute left-3 right-3 bottom-full mb-2 max-h-72 overflow-y-auto rounded-lg border border-border-theme bg-white py-1 shadow-[0_8px_30px_rgb(0,0,0,0.12)] z-50">
          {slashSections.map((section, sectionIndex) => {
            let baseIndex = 0;
            for (let i = 0; i < sectionIndex; i += 1) {
              baseIndex += slashSections[i].items.length;
            }

            return (
              <div key={section.key} className={sectionIndex > 0 ? "mt-1" : ""}>
                <div className="px-3 py-1.5 text-[11px] font-medium uppercase tracking-wide text-text-secondary">
                  {section.label}
                </div>
                {section.items.map((cmd, index) => {
                  const flatIndex = baseIndex + index;
                  return (
                    <button
                      key={cmd.id}
                      type="button"
                      onMouseDown={(e) => {
                        e.preventDefault();
                        chooseSlash(cmd);
                      }}
                      onMouseEnter={() => setSlashSelected(flatIndex)}
                      className={`grid w-full grid-cols-[150px_minmax(0,1fr)_72px] items-center gap-3 px-3 py-2 text-left text-[12px] transition-colors ${
                        flatIndex === slashSelected ? "bg-gray-100 text-text-base" : "text-text-secondary"
                      }`}
                    >
                      <span className="truncate font-medium text-text-base">{cmd.title}</span>
                      <span className="truncate text-[12px] leading-snug text-text-secondary">
                        {cmd.description}
                      </span>
                      <span className="truncate text-right text-[11px] text-text-secondary">
                        {cmd.category}
                      </span>
                    </button>
                  );
                })}
              </div>
            );
          })}
        </div>
      )}
      {planMode && (
        <div className="mb-2 inline-flex w-fit items-center rounded-md border border-amber-200 bg-amber-50 px-2 py-1 text-[11px] font-medium text-amber-700">
          <FontAwesomeIcon icon={["fas", "list-check"]} className="mr-1.5 text-[10px]" />
          Plan Mode
        </div>
      )}

      {attachments.length > 0 && (
        <div className="mb-2 flex max-w-full gap-2 overflow-x-auto pb-1">
          {attachments.map((item) => (
            <div
              key={item.id}
              className="flex h-14 max-w-[220px] shrink-0 items-center gap-2 rounded-xl border border-border-theme bg-gray-50 px-2 text-left"
            >
              {item.kind === "image" && item.dataUrl ? (
                <img src={item.dataUrl} alt="" className="h-10 w-10 rounded-lg object-cover" />
              ) : (
                <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-white text-text-secondary">
                  <FontAwesomeIcon icon={["fas", item.kind === "text" ? "file-lines" : "file"]} />
                </div>
              )}
              <div className="min-w-0 flex-1">
                <div className="truncate text-[12px] font-medium text-text-base">{item.name}</div>
                <div className="truncate text-[11px] text-text-secondary">
                  {item.status === "processing"
                    ? item.kind === "image"
                      ? "识别中"
                      : "处理中"
                    : item.status === "error"
                    ? item.error ?? "处理失败"
                    : item.kind === "image"
                    ? "图片"
                    : item.kind === "text"
                    ? "文本"
                    : "文件"}
                </div>
              </div>
              <button
                type="button"
                onClick={() => removeAttachment(item.id)}
                className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-text-secondary hover:bg-gray-200 hover:text-text-base"
              >
                <FontAwesomeIcon icon={["fas", "xmark"]} className="text-[11px]" />
              </button>
            </div>
          ))}
        </div>
      )}

      <div className="relative w-full">
        <div
          ref={mirrorRef}
          aria-hidden="true"
          className="pointer-events-none absolute inset-0 overflow-hidden custom-scrollbar whitespace-pre-wrap break-words bg-transparent text-sm leading-6 text-text-base"
          style={{
            minHeight: `${COMPOSER_TEXTAREA_MIN_HEIGHT}px`,
            maxHeight: `${textareaMaxHeight}px`,
          }}
        >
          {mirrorContent}
        </div>
        <textarea
          ref={textareaRef}
          aria-label={editorPlaceholder}
          className="composer-textarea custom-scrollbar relative z-10 w-full resize-none bg-transparent text-sm leading-6 text-transparent caret-text-base outline-none placeholder-transparent"
          style={{
            minHeight: `${COMPOSER_TEXTAREA_MIN_HEIGHT}px`,
            maxHeight: `${textareaMaxHeight}px`,
          }}
          placeholder={editorPlaceholder}
          value={draftValue}
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

      <div className="flex flex-wrap items-center justify-between mt-2 pt-1 gap-y-2">
        <div className="flex flex-wrap items-center gap-2">
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
          <button
            type="button"
            onClick={() => fileInputRef.current?.click()}
            disabled={attachments.length >= MAX_COMPOSER_ATTACHMENTS}
            title={
              attachments.length >= MAX_COMPOSER_ATTACHMENTS
                ? `Maximum ${MAX_COMPOSER_ATTACHMENTS} attachments allowed`
                : undefined
            }
            className="w-7 h-7 flex-shrink-0 rounded flex items-center justify-center text-text-secondary hover:bg-gray-100 transition-colors disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent"
          >
            <FontAwesomeIcon icon={["fas", "plus"]} />
          </button>
          
          {visibleOptions.length > 0 && (
            <div className="relative" ref={approvalDropdownRef}>
              <div 
                className="flex items-center flex-shrink-0 whitespace-nowrap text-blue-500 text-xs font-medium cursor-pointer hover:bg-blue-50 px-2 py-1.5 rounded transition-colors"
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

        <div className="flex flex-wrap items-center gap-2">
          <div className="relative" ref={thinkingDropdownRef}>
            <div
              className="flex items-center flex-shrink-0 whitespace-nowrap bg-gray-50 border border-border-theme rounded-full px-2.5 py-1 cursor-pointer hover:bg-gray-100 transition-colors text-xs text-text-base"
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
              className="flex items-center flex-shrink-0 whitespace-nowrap bg-gray-50 border border-border-theme rounded-full px-3 py-1 cursor-pointer hover:bg-gray-100 transition-colors text-xs text-text-base"
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
                submitWithAttachments();
              }
            }}
            disabled={busy && !onStop}
            title={busy ? t("composer.stop") : undefined}
            className={`w-8 h-8 flex-shrink-0 rounded-full text-white flex items-center justify-center transition-colors ${
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



