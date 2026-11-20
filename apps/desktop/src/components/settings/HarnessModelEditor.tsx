import { useState, type FormEvent } from "react";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";
import { HARNESS_PROTOCOLS, HARNESS_THINKING_LEVELS, type HarnessModelInput, type HarnessModelView,
  type HarnessProtocol, type HarnessThinkingLevel, type HarnessReasoningEfforts, type HarnessThinkingFormat } from "../../harnessModelsApi";

const LEVEL_LABEL = { off: "关闭", low: "低", medium: "中", high: "高", max: "最高" };
const MODE_LABEL = { inherit: "自动匹配能力", disabled: "无思考控件", custom: "自定义档位" };
type ReasoningMode = keyof typeof MODE_LABEL;
const FORMAT_LABEL = { auto: "自动匹配协议", effort_only: "仅强度参数", enabled: "启用思考与强度参数", adaptive: "自适应思考与强度参数", budget: "思考 Token 预算" };
const PROTOCOL_LABEL = { responses: "Responses", chat_completions: "Chat Completions", messages: "Messages" };

type Props = {
  model: HarnessModelView;
  onSave: (input: HarnessModelInput) => Promise<void>;
  onCancel: () => void;
};

export function HarnessModelEditor({ model, onSave, onCancel }: Props) {
  const [name, setName] = useState(model.displayName);
  const [protocol, setProtocol] = useState<HarnessProtocol | null>(model.protocol);
  const [context, setContext] = useState(String(model.contextWindow));
  const [output, setOutput] = useState(String(model.outputLimit));
  const [mode, setMode] = useState<ReasoningMode>(model.reasoningEfforts === undefined
    ? "inherit" : model.reasoningEfforts === false ? "disabled" : "custom");
  const [efforts, setEfforts] = useState<Exclude<HarnessReasoningEfforts, false>>(
    model.reasoningEfforts || model.effectiveReasoningEfforts || { off: null, low: "low", medium: "medium", high: "high" });
  const initialEfforts = model.reasoningEfforts || model.effectiveReasoningEfforts;
  const [defaultEffort, setDefaultEffort] = useState<HarnessThinkingLevel>(
    model.defaultReasoningEffort ?? model.effectiveDefaultReasoningEffort
    ?? HARNESS_THINKING_LEVELS.find(level => level !== "off" && initialEfforts && Object.prototype.hasOwnProperty.call(initialEfforts, level)) ?? "high");
  const [format, setFormat] = useState<HarnessThinkingFormat | "auto">(model.thinkingFormat ?? "auto");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const offeredEfforts = mode === "custom" ? efforts : mode === "inherit" ? model.effectiveReasoningEfforts : false;

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (saving) return;
    const contextWindow = Number(context);
    const outputLimit = Number(output);
    if (!name.trim() || !Number.isSafeInteger(contextWindow) || contextWindow <= 0
      || !Number.isSafeInteger(outputLimit) || outputLimit <= 0 || outputLimit > 0xffffffff) {
      setError("填写模型名称与有效的正整数容量");
      return;
    }
    if (mode === "custom" && (!Object.keys(efforts).some(level => level !== "off")
      || Object.entries(efforts).some(([level, wire]) => wire === null ? level !== "off" : !wire?.trim()))) {
      setError("至少配置一个思考档位，API 参数值不能为空；关闭档位留空表示省略参数");
      return;
    }
    if (mode !== "disabled"
      && (!offeredEfforts || !Object.prototype.hasOwnProperty.call(offeredEfforts, defaultEffort))) {
      setError("默认档位必须是已启用的思考档位");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await onSave({
        providerId: model.providerId, modelName: model.modelName, displayName: name.trim(),
        protocol, contextWindow, outputLimit,
        ...(mode === "inherit" ? {} : { reasoningEfforts: mode === "disabled" ? false : efforts }),
        defaultReasoningEffort: mode === "disabled" ? null : defaultEffort,
        thinkingFormat: format === "auto" ? null : format,
      });
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setSaving(false);
    }
  };

  return (
    <form onSubmit={save} className="flex min-h-0 flex-col gap-5 overflow-y-auto px-6 pb-6">
      {error && <div role="alert" className="text-sm text-red-700">{error}</div>}
      <div className="grid grid-cols-2 gap-4">
        <div className="col-span-2 flex flex-col gap-1.5">
          <Label htmlFor="model-name">显示名称</Label>
          <Input id="model-name" value={name} disabled={saving} onChange={event => setName(event.target.value)} />
        </div>
        <div className="col-span-2 flex flex-col gap-1.5">
          <Label>协议</Label>
          <Select value={protocol ?? "inherit"} onValueChange={value => setProtocol(value === "inherit" ? null : value as HarnessProtocol)}>
            <SelectTrigger className="rounded-lg bg-hover-bg" disabled={saving}><SelectValue placeholder={protocol ? PROTOCOL_LABEL[protocol] : "继承供应商"} /></SelectTrigger>
            <SelectContent>
              <SelectItem value="inherit">继承供应商</SelectItem>
              {HARNESS_PROTOCOLS.map(value => <SelectItem key={value} value={value}>{PROTOCOL_LABEL[value]}</SelectItem>)}
            </SelectContent>
          </Select>
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="model-context">上下文窗口</Label>
          <Input id="model-context" inputMode="numeric" value={context} disabled={saving} onChange={event => setContext(event.target.value)} />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="model-output">输出上限</Label>
          <Input id="model-output" inputMode="numeric" value={output} disabled={saving} onChange={event => setOutput(event.target.value)} />
        </div>
      </div>
      <div className="flex flex-col gap-2">
        <Label htmlFor="model-reasoning-mode">思考强度</Label>
        <Select value={mode} onValueChange={value => setMode(value as ReasoningMode)}>
          <SelectTrigger id="model-reasoning-mode" disabled={saving}><SelectValue placeholder={MODE_LABEL[mode]} /></SelectTrigger>
          <SelectContent>
            {Object.entries(MODE_LABEL).map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      {mode !== "disabled" && <div className="flex flex-col gap-2">
        <Label>参数格式</Label>
        <Select value={format} onValueChange={value => setFormat(value as typeof format)}>
          <SelectTrigger disabled={saving}><SelectValue placeholder={FORMAT_LABEL[format]} /></SelectTrigger>
          <SelectContent>{Object.entries(FORMAT_LABEL).map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}</SelectContent>
        </Select>
      </div>}
      {mode === "custom" && <fieldset className="space-y-2" disabled={saving}>
        <legend className="mb-2 text-sm font-medium">档位 / API 参数值</legend>
        {HARNESS_THINKING_LEVELS.map((level: HarnessThinkingLevel) => {
          const enabled = Object.prototype.hasOwnProperty.call(efforts, level);
          return <div key={level} className="grid grid-cols-[108px_1fr] items-center gap-3">
            <Label className="flex items-center gap-2" htmlFor={`model-level-${level}`}>
              <Checkbox id={`model-level-${level}`} checked={enabled} disabled={saving} onCheckedChange={checked => setEfforts(current => {
                const next = { ...current };
                if (checked === true) next[level] = level === "off" ? null : level;
                else delete next[level];
                return next;
              })} />
              {LEVEL_LABEL[level]} · {level}
            </Label>
            <Input id={`model-wire-${level}`} aria-label={`${level} API 参数值`} value={efforts[level] ?? ""}
              placeholder={level === "off" ? "省略参数" : level} disabled={saving || !enabled}
              onChange={event => setEfforts(current => ({ ...current,
                [level]: level === "off" && !event.target.value.trim() ? null : event.target.value.trim(),
              }))} />
          </div>;
        })}
      </fieldset>}
      {mode !== "disabled" && <div className="flex flex-col gap-2">
        <Label htmlFor="model-reasoning-default">默认档位</Label>
        <Select value={defaultEffort} onValueChange={value => setDefaultEffort(value as typeof defaultEffort)}>
          <SelectTrigger id="model-reasoning-default" disabled={saving}>
            <SelectValue placeholder={LEVEL_LABEL[defaultEffort]} />
          </SelectTrigger>
          <SelectContent>
            {HARNESS_THINKING_LEVELS.filter(level => offeredEfforts && Object.prototype.hasOwnProperty.call(offeredEfforts, level))
              .map(level => <SelectItem key={level} value={level}>{LEVEL_LABEL[level]} · {level}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>}
      <div className="flex justify-end gap-2">
        <Button variant="outline" onClick={onCancel} disabled={saving}>取消</Button>
        <Button type="submit" disabled={saving}>{saving ? "保存中" : "保存"}</Button>
      </div>
    </form>
  );
}
