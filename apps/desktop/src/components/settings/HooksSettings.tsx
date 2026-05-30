import { useEffect, useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { getHooksJson, setHooksJson } from "../../api";

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

/** Count declared hook actions for a quick summary; null when not parseable. */
function countActions(json: string): number | null {
  if (json.trim() === "") return 0;
  try {
    const parsed = JSON.parse(json) as {
      hooks?: Record<string, { hooks?: unknown[] }[]>;
    };
    if (!parsed.hooks) return 0;
    let n = 0;
    for (const groups of Object.values(parsed.hooks)) {
      for (const g of groups) n += g.hooks?.length ?? 0;
    }
    return n;
  } catch {
    return null;
  }
}

export function HooksSettings() {
  const { t } = useTranslation();
  const [value, setValue] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [status, setStatus] = useState<"idle" | "saved" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");

  useEffect(() => {
    getHooksJson()
      .then((j) => {
        setValue(j);
        setLoaded(true);
      })
      .catch(() => setLoaded(true));
  }, []);

  const actionCount = useMemo(() => countActions(value), [value]);
  const valid = actionCount !== null;

  const save = async () => {
    try {
      await setHooksJson(value);
      setStatus("saved");
      setErrorMsg("");
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
    } catch {
      setStatus("error");
      setErrorMsg(t("settings.hooks.invalidJson"));
    }
  };

  return (
    <>
      <div className="mb-8 max-w-[700px]">
        <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.hooks.title")}</h1>
        <div className="flex items-center justify-between text-[13px] text-text-secondary">
          <div>
            {t("settings.hooks.desc")}{" "}
            <a href="#" className="text-blue-500 hover:underline">
              {t("settings.hooks.learnMore")}
            </a>
          </div>
        </div>
      </div>

      <div className="max-w-[700px] border border-border-theme rounded-xl bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] p-4">
        <div className="flex items-center justify-between mb-2">
          <div className="text-[13px] font-medium text-text-base font-mono">
            {t("settings.hooks.editorTitle")}
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={format}
              className="text-[12px] px-2.5 py-1 rounded-md border border-border-theme text-text-secondary hover:bg-gray-50 transition-colors"
            >
              {t("settings.hooks.format")}
            </button>
            <button
              onClick={save}
              disabled={!valid || !loaded}
              className="text-[12px] px-3 py-1 rounded-md bg-text-base text-white hover:opacity-90 transition-opacity disabled:opacity-40"
            >
              {t("settings.hooks.save")}
            </button>
          </div>
        </div>

        <div className="text-[12px] text-text-secondary leading-relaxed mb-3">
          {t("settings.hooks.editorDesc")}
        </div>

        <textarea
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            setStatus("idle");
          }}
          spellCheck={false}
          rows={16}
          placeholder={PLACEHOLDER}
          className={`w-full font-mono text-[12px] leading-relaxed border rounded-lg px-3 py-2 outline-none resize-y transition-colors ${
            valid
              ? "border-border-theme focus:border-blue-400"
              : "border-red-400 focus:border-red-500"
          }`}
        />

        <div className="flex items-center justify-between mt-2 text-[12px]">
          <div className="text-text-secondary">
            {valid ? (
              <span className="flex items-center gap-1.5">
                <FontAwesomeIcon icon={["fas", "circle-check"]} className="text-green-500" />
                {t("settings.hooks.actionsRegistered", { count: actionCount ?? 0 })}
              </span>
            ) : (
              <span className="flex items-center gap-1.5 text-red-500">
                <FontAwesomeIcon icon={["fas", "circle-info"]} />
                {t("settings.hooks.invalidJson")}
              </span>
            )}
          </div>
          {status === "saved" && (
            <span className="text-green-600 flex items-center gap-1.5">
              <FontAwesomeIcon icon={["fas", "circle-check"]} />
              {t("settings.hooks.saved")}
            </span>
          )}
          {status === "error" && errorMsg && (
            <span className="text-red-500 truncate max-w-[360px]" title={errorMsg}>
              {errorMsg}
            </span>
          )}
        </div>
      </div>
    </>
  );
}
