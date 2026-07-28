import { useEffect, useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import {
  getHooksJson,
  listEffectiveHooks,
  setHooksJson,
  testHookCommand,
  type EffectiveHookGroup,
  type TestHookCommandResult,
} from "../../api";

const PLACEHOLDER = `{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "python3 validate.py", "timeout": 10 }
        ]
      }
    ]
  }
}`;

type HookAction = {
  type?: string;
  command?: string;
  prompt?: string;
  model?: string;
  agent?: string;
  arguments?: Record<string, unknown>;
  url?: string;
  timeout?: number;
  shell?: string;
  env?: Record<string, string>;
};

type HookGroup = {
  matcher?: string;
  hooks?: HookAction[];
};

type HookListItem = {
  source?: string;
  event: string;
  matcher?: string;
  actions: HookAction[];
};

type TestHookTarget = {
  event: string;
  matcher?: string;
  action: HookAction & { command: string };
};

type ParsedHooks =
  | { valid: true; items: HookListItem[]; actionCount: number }
  | { valid: false; items: HookListItem[]; actionCount: null; error: string };

function parseHookItems(json: string): ParsedHooks {
  if (json.trim() === "") {
    return { valid: true, items: [], actionCount: 0 };
  }

  try {
    const parsed = JSON.parse(json) as {
      hooks?: Record<string, HookGroup[]>;
    };
    const items: HookListItem[] = [];

    for (const [event, groups] of Object.entries(parsed.hooks ?? {})) {
      if (!Array.isArray(groups)) continue;
      for (const group of groups) {
        const actions = Array.isArray(group.hooks) ? group.hooks : [];
        items.push({
          event,
          matcher: typeof group.matcher === "string" ? group.matcher : undefined,
          actions,
        });
      }
    }

    return {
      valid: true,
      items,
      actionCount: items.reduce((sum, item) => sum + item.actions.length, 0),
    };
  } catch (error) {
    return {
      valid: false,
      items: [],
      actionCount: null,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

type HooksDoc = { hooks?: Record<string, HookGroup[]> };

function safeParseDoc(json: string): HooksDoc {
  if (json.trim() === "") return { hooks: {} };
  try {
    const parsed = JSON.parse(json) as HooksDoc;
    return parsed && typeof parsed === "object" ? parsed : { hooks: {} };
  } catch {
    return { hooks: {} };
  }
}

/** Append every group from `incoming` into `base`, per event (add-only save). */
function mergeHooksJson(baseJson: string, incomingJson: string): string {
  const base = safeParseDoc(baseJson);
  const incoming = safeParseDoc(incomingJson);
  const merged: Record<string, HookGroup[]> = { ...(base.hooks ?? {}) };
  for (const [event, groups] of Object.entries(incoming.hooks ?? {})) {
    if (!Array.isArray(groups)) continue;
    merged[event] = [...(merged[event] ?? []), ...groups];
  }
  return JSON.stringify({ hooks: merged }, null, 2);
}

/** Remove the group at `groupIndex` within `event` from the persisted doc. */
function removeHookGroup(baseJson: string, event: string, groupIndex: number): string {
  const base = safeParseDoc(baseJson);
  const hooks: Record<string, HookGroup[]> = { ...(base.hooks ?? {}) };
  const groups = Array.isArray(hooks[event]) ? [...hooks[event]] : [];
  groups.splice(groupIndex, 1);
  if (groups.length === 0) {
    delete hooks[event];
  } else {
    hooks[event] = groups;
  }
  return JSON.stringify({ hooks }, null, 2);
}

/** Parse persisted hooks into list items carrying their (event, groupIndex)
 * so the saved-hooks list can delete a specific group. */
function userItemsFromJson(json: string): (HookListItem & { groupIndex: number })[] {
  const doc = safeParseDoc(json);
  const items: (HookListItem & { groupIndex: number })[] = [];
  for (const [event, groups] of Object.entries(doc.hooks ?? {})) {
    if (!Array.isArray(groups)) continue;
    groups.forEach((group, groupIndex) => {
      items.push({
        source: "user",
        event,
        matcher: typeof group.matcher === "string" ? group.matcher : undefined,
        actions: Array.isArray(group.hooks) ? group.hooks : [],
        groupIndex,
      });
    });
  }
  return items;
}

function primaryCommand(actions: HookAction[]): string {
  const firstCommand = actions.find((action) => action.command?.trim())?.command?.trim();
  if (firstCommand) return firstCommand;
  const firstUrl = actions.find((action) => action.url?.trim())?.url?.trim();
  if (firstUrl) return firstUrl;
  const firstPrompt = actions.find((action) => action.prompt?.trim())?.prompt?.trim();
  if (firstPrompt) return firstPrompt;
  return actions.length > 0 ? `${actions[0]?.type ?? "command"} action` : "无 action";
}

function findFirstCommandHook(items: HookListItem[]): TestHookTarget | null {
  for (const item of items) {
    for (const action of item.actions) {
      const command = action.command?.trim();
      if (command && (action.type ?? "command") === "command") {
        return {
          event: item.event,
          matcher: item.matcher,
          action: { ...action, type: action.type ?? "command", command },
        };
      }
    }
  }
  return null;
}

function compactText(value: string, max = 180): string {
  const singleLine = value.replace(/\s+/g, " ").trim();
  return singleLine.length > max ? `${singleLine.slice(0, max - 1)}...` : singleLine;
}

function testResultText(result: TestHookCommandResult): string {
  const detail = compactText(result.stderr || result.stdout || result.outcome);
  return `测试：${result.outcome} · exit ${result.exit_code} · ${result.duration_ms}ms${
    detail ? ` · ${detail}` : ""
  }`;
}

function sourceLabel(source?: string): string {
  if (source === "user") return "用户";
  if (source === "plugin") return "插件";
  return source ?? "";
}

function effectiveToListItem(group: EffectiveHookGroup): HookListItem {
  return {
    source: group.source,
    event: group.event,
    matcher: group.matcher ?? undefined,
    actions: group.actions.map((action) => ({
      type: action.action_type,
      command: action.command,
      prompt: action.prompt,
      model: action.model ?? undefined,
      agent: action.agent ?? undefined,
      arguments: action.arguments ?? undefined,
      url: action.url,
      timeout: action.timeout ?? undefined,
      shell: action.shell,
    })),
  };
}

function HookList({
  items,
  onDelete,
}: {
  items: (HookListItem & { groupIndex?: number })[];
  onDelete?: (item: HookListItem & { groupIndex?: number }) => void;
}) {
  if (items.length === 0) {
    return <div className="py-2 text-[12px] text-text-secondary">暂无保存的钩子</div>;
  }

  return (
    <ul className="divide-y divide-border-theme border-y border-border-theme">
      {items.map((item, index) => (
        <li
          key={`${item.event}-${item.matcher ?? "all"}-${index}`}
          className="flex items-center justify-between gap-4 py-2.5"
        >
          <div className="min-w-0">
            <div className="flex min-w-0 items-center gap-2">
              <span className="truncate font-mono text-[12px] font-medium text-text-base">
                {item.event}
              </span>
              {item.source && (
                <span className="shrink-0 text-[11px] text-text-secondary">
                  {sourceLabel(item.source)}
                </span>
              )}
              {item.matcher && (
                <span className="shrink-0 font-mono text-[11px] text-text-secondary">
                  {item.matcher}
                </span>
              )}
              {item.actions[0]?.shell && item.actions[0]?.shell !== "auto" && (
                <span className="shrink-0 font-mono text-[11px] text-text-secondary">
                  {item.actions[0].shell}
                </span>
              )}
            </div>
            <div className="mt-0.5 truncate font-mono text-[12px] text-text-secondary">
              {primaryCommand(item.actions)}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-3">
            <span className="text-[11px] text-text-secondary">
              {item.actions.length} actions
            </span>
            {onDelete && item.source === "user" && (
              <button
                onClick={() => onDelete(item)}
                className="rounded-md border border-border-theme px-2 py-0.5 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-500"
                title="删除这条钩子"
              >
                删除
              </button>
            )}
          </div>
        </li>
      ))}
    </ul>
  );
}

export function HooksSettings() {
  const { t } = useTranslation();
  // The textarea is an ADD-only tool: it starts empty, and clicking Save
  // appends its groups to the persisted set then clears it. Managing (viewing
  // / deleting) saved hooks happens in the list below, not by editing this
  // box. `savedJson` is the persisted hooks.json (DB-backed) that drives the
  // saved list and is the merge base on save.
  const [value, setValue] = useState("");
  const [savedJson, setSavedJson] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [status, setStatus] = useState<"idle" | "saved" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");
  const [pluginItems, setPluginItems] = useState<HookListItem[]>([]);
  const [builtinHookCount, setBuiltinHookCount] = useState(0);
  const [effectiveErrors, setEffectiveErrors] = useState<string[]>([]);
  const [testStatus, setTestStatus] = useState<"idle" | "running" | "done" | "error">("idle");
  const [testResult, setTestResult] = useState<TestHookCommandResult | null>(null);
  const [testError, setTestError] = useState("");

  const refreshEffectiveHooks = async () => {
    try {
      const effective = await listEffectiveHooks();
      setPluginItems(effective.plugin_hooks.map(effectiveToListItem));
      setBuiltinHookCount(effective.builtin_hooks.length);
      setEffectiveErrors(effective.errors);
    } catch (error) {
      setEffectiveErrors([String(error)]);
    }
  };

  useEffect(() => {
    Promise.all([getHooksJson(), listEffectiveHooks()])
      .then(([j, effective]) => {
        // Persisted hooks drive the saved list; the add box stays empty.
        setSavedJson(j);
        setPluginItems(effective.plugin_hooks.map(effectiveToListItem));
        setBuiltinHookCount(effective.builtin_hooks.length);
        setEffectiveErrors(effective.errors);
        setLoaded(true);
      })
      .catch(() => setLoaded(true));
  }, []);

  const parsedHooks = useMemo(() => parseHookItems(value), [value]);
  const valid = parsedHooks.valid;
  const hasInput = value.trim() !== "";
  const userItems = useMemo(() => userItemsFromJson(savedJson), [savedJson]);
  const testTarget = useMemo(
    () => (parsedHooks.valid ? findFirstCommandHook(parsedHooks.items) : null),
    [parsedHooks],
  );

  const save = async () => {
    try {
      // Add-only: merge the input's groups into the persisted set, persist the
      // merged JSON, then clear the box so it is ready for the next addition.
      const merged = mergeHooksJson(savedJson, value);
      await setHooksJson(merged);
      setSavedJson(merged);
      setValue("");
      setStatus("saved");
      setErrorMsg("");
      await refreshEffectiveHooks();
      setTimeout(() => setStatus("idle"), 1800);
    } catch (e) {
      setStatus("error");
      setErrorMsg(String(e));
    }
  };

  const deleteHook = async (item: HookListItem & { groupIndex?: number }) => {
    if (item.source !== "user" || item.groupIndex === undefined) return;
    try {
      const next = removeHookGroup(savedJson, item.event, item.groupIndex);
      await setHooksJson(next);
      setSavedJson(next);
      setStatus("saved");
      setErrorMsg("");
      await refreshEffectiveHooks();
      setTimeout(() => setStatus("idle"), 1800);
    } catch (e) {
      setStatus("error");
      setErrorMsg(String(e));
    }
  };

  const format = () => {
    try {
      setValue(JSON.stringify(JSON.parse(value), null, 2));
      setStatus("idle");
      setTestStatus("idle");
      setTestResult(null);
      setTestError("");
    } catch {
      setStatus("error");
      setErrorMsg(t("settings.hooks.invalidJson"));
    }
  };

  const testFirst = async () => {
    if (!testTarget) {
      setTestStatus("error");
      setTestError("没有可测试的 command hook");
      setTestResult(null);
      return;
    }

    try {
      setTestStatus("running");
      setTestError("");
      const result = await testHookCommand({
        event: testTarget.event,
        matcher: testTarget.matcher ?? null,
        action: {
          type: testTarget.action.type ?? "command",
          command: testTarget.action.command,
          timeout: testTarget.action.timeout ?? null,
          shell: testTarget.action.shell ?? "auto",
          env: testTarget.action.env ?? {},
        },
      });
      setTestResult(result);
      setTestStatus("done");
    } catch (error) {
      setTestStatus("error");
      setTestError(String(error));
      setTestResult(null);
    }
  };

  return (
    <>
      <div className="mb-8 max-w-[700px]">
        <h1 className="mb-1 text-2xl font-semibold text-text-base">
          {t("settings.hooks.title")}
        </h1>
        <div className="flex items-center justify-between text-[13px] text-text-secondary">
          <div>
            {t("settings.hooks.desc")}{" "}
            <a href="#" className="text-blue-500 hover:underline">
              {t("settings.hooks.learnMore")}
            </a>
          </div>
        </div>
      </div>

      <div className="max-w-[700px] border-t border-border-theme pt-4">
        <div className="mb-2 flex items-center justify-between">
          <div className="font-mono text-[13px] font-medium text-text-base">
            {t("settings.hooks.editorTitle")}
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={testFirst}
              disabled={!valid || !testTarget || testStatus === "running"}
              className="rounded-md border border-border-theme bg-transparent px-2.5 py-1 text-[12px] text-text-secondary transition-colors hover:bg-hover-bg hover:text-text-base disabled:opacity-40"
              title={testTarget ? primaryCommand([testTarget.action]) : "没有可测试的 command hook"}
            >
              {testStatus === "running" ? "测试中..." : "测试第一条"}
            </button>
            <button
              onClick={format}
              className="rounded-md border border-border-theme px-2.5 py-1 text-[12px] text-text-secondary transition-colors hover:bg-hover-bg hover:text-text-base"
            >
              {t("settings.hooks.format")}
            </button>
            <button
              onClick={save}
              disabled={!valid || !loaded || !hasInput}
              className="rounded-md bg-text-base px-3 py-1 text-[12px] text-white transition-opacity hover:opacity-90 disabled:opacity-40"
              title={hasInput ? undefined : "输入框仅用于新增：粘贴一段 hooks JSON 后保存，会追加到下方已保存列表"}
            >
              {t("settings.hooks.save")}
            </button>
          </div>
        </div>

        <div className="mb-3 text-[12px] leading-relaxed text-text-secondary">
          {t("settings.hooks.editorDesc")}
        </div>

        <textarea
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            setStatus("idle");
            setTestStatus("idle");
            setTestResult(null);
            setTestError("");
          }}
          spellCheck={false}
          rows={16}
          placeholder={PLACEHOLDER}
          className={`w-full resize-y rounded-md border bg-bg-base px-3 py-2 font-mono text-[12px] leading-relaxed text-text-base outline-none transition-colors ${
            valid
              ? "border-border-theme focus:border-blue-400"
              : "border-red-400 focus:border-red-500"
          }`}
        />

        <div className="mt-2 flex items-center justify-between text-[12px]">
          <div className="text-text-secondary">
            {valid ? (
              <span className="flex items-center gap-1.5">
                <FontAwesomeIcon icon={["fas", "circle-check"]} className="text-green-500" />
                {t("settings.hooks.actionsRegistered", {
                  count: parsedHooks.actionCount ?? 0,
                })}
              </span>
            ) : (
              <span className="flex items-center gap-1.5 text-red-500">
                <FontAwesomeIcon icon={["fas", "circle-info"]} />
                {t("settings.hooks.invalidJson")}
              </span>
            )}
          </div>
          {status === "saved" && (
            <span className="flex items-center gap-1.5 text-green-600">
              <FontAwesomeIcon icon={["fas", "circle-check"]} />
              {t("settings.hooks.saved")}
            </span>
          )}
          {status === "error" && errorMsg && (
            <span className="max-w-[360px] truncate text-red-500" title={errorMsg}>
              {errorMsg}
            </span>
          )}
        </div>

        {(testResult || testError) && (
          <div
            className={`mt-2 truncate text-[12px] ${
              testStatus === "error"
                ? "text-red-500"
                : testResult?.outcome === "blocked"
                  ? "text-amber-600"
                  : "text-text-secondary"
            }`}
            title={testError || (testResult ? testResultText(testResult) : undefined)}
          >
            {testError || (testResult ? testResultText(testResult) : "")}
          </div>
        )}
      </div>

      <div className="mt-5 max-w-[700px]">
        <div className="mb-2 flex items-center justify-between">
          <div className="text-[13px] font-medium text-text-base">已保存的钩子</div>
          <div className="text-[11px] text-text-secondary">
            {userItems.length + pluginItems.length} groups
            {builtinHookCount > 0 ? ` · 内置 ${builtinHookCount}` : ""}
          </div>
        </div>
        <HookList items={[...userItems, ...pluginItems]} onDelete={deleteHook} />
        {effectiveErrors.length > 0 && (
          <div className="mt-2 space-y-1 text-[12px] text-red-500">
            {effectiveErrors.map((error, index) => (
              <div key={`${error}-${index}`}>{error}</div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}
