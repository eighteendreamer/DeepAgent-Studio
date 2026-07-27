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
  timeout?: number;
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

function primaryCommand(actions: HookAction[]): string {
  const firstCommand = actions.find((action) => action.command?.trim())?.command?.trim();
  if (firstCommand) return firstCommand;
  return actions.length > 0 ? "command action" : "无 action";
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
      timeout: action.timeout ?? undefined,
    })),
  };
}

function HookList({ items }: { items: HookListItem[] }) {
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
            </div>
            <div className="mt-0.5 truncate font-mono text-[12px] text-text-secondary">
              {primaryCommand(item.actions)}
            </div>
          </div>
          <div className="shrink-0 text-[11px] text-text-secondary">
            {item.actions.length} actions
          </div>
        </li>
      ))}
    </ul>
  );
}

export function HooksSettings() {
  const { t } = useTranslation();
  const [value, setValue] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [status, setStatus] = useState<"idle" | "saved" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");
  const [savedHookItems, setSavedHookItems] = useState<HookListItem[]>([]);
  const [builtinHookCount, setBuiltinHookCount] = useState(0);
  const [effectiveErrors, setEffectiveErrors] = useState<string[]>([]);
  const [testStatus, setTestStatus] = useState<"idle" | "running" | "done" | "error">("idle");
  const [testResult, setTestResult] = useState<TestHookCommandResult | null>(null);
  const [testError, setTestError] = useState("");

  const refreshEffectiveHooks = async () => {
    try {
      const effective = await listEffectiveHooks();
      setSavedHookItems(
        [...effective.user_hooks, ...effective.plugin_hooks].map(effectiveToListItem),
      );
      setBuiltinHookCount(effective.builtin_hooks.length);
      setEffectiveErrors(effective.errors);
    } catch (error) {
      setEffectiveErrors([String(error)]);
    }
  };

  useEffect(() => {
    Promise.all([getHooksJson(), listEffectiveHooks()])
      .then(([j, effective]) => {
        setValue(j);
        setSavedHookItems(
          [...effective.user_hooks, ...effective.plugin_hooks].map(effectiveToListItem),
        );
        setBuiltinHookCount(effective.builtin_hooks.length);
        setEffectiveErrors(effective.errors);
        setLoaded(true);
      })
      .catch(() => setLoaded(true));
  }, []);

  const parsedHooks = useMemo(() => parseHookItems(value), [value]);
  const valid = parsedHooks.valid;
  const testTarget = useMemo(
    () => (parsedHooks.valid ? findFirstCommandHook(parsedHooks.items) : null),
    [parsedHooks],
  );

  const save = async () => {
    try {
      await setHooksJson(value);
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

      <div className="max-w-[700px] rounded-xl border border-border-theme bg-white p-4 shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
        <div className="mb-2 flex items-center justify-between">
          <div className="font-mono text-[13px] font-medium text-text-base">
            {t("settings.hooks.editorTitle")}
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={testFirst}
              disabled={!valid || !testTarget || testStatus === "running"}
              className="rounded-md border border-border-theme bg-transparent px-2.5 py-1 text-[12px] text-text-secondary transition-colors hover:bg-gray-50 hover:text-text-base disabled:opacity-40"
              title={testTarget ? primaryCommand([testTarget.action]) : "没有可测试的 command hook"}
            >
              {testStatus === "running" ? "测试中..." : "测试第一条"}
            </button>
            <button
              onClick={format}
              className="rounded-md border border-border-theme px-2.5 py-1 text-[12px] text-text-secondary transition-colors hover:bg-gray-50"
            >
              {t("settings.hooks.format")}
            </button>
            <button
              onClick={save}
              disabled={!valid || !loaded}
              className="rounded-md bg-text-base px-3 py-1 text-[12px] text-white transition-opacity hover:opacity-90 disabled:opacity-40"
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
          className={`w-full resize-y rounded-lg border px-3 py-2 font-mono text-[12px] leading-relaxed outline-none transition-colors ${
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
            {savedHookItems.length} groups
            {builtinHookCount > 0 ? ` · 内置 ${builtinHookCount}` : ""}
          </div>
        </div>
        <HookList items={savedHookItems} />
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
