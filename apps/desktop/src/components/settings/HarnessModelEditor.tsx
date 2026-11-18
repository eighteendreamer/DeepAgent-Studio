import { useState, type FormEvent } from "react";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";
import { HARNESS_PROTOCOLS, type HarnessModelInput, type HarnessModelView, type HarnessProtocol } from "../../harnessModelsApi";

const LEVELS = ["off", "low", "medium", "high", "max"] as const;
const LEVEL_LABEL = { off: "关闭", low: "低", medium: "中", high: "高", max: "最高" };
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
  const [levels, setLevels] = useState(model.thinkingLevels);
  const [distinct, setDistinct] = useState(model.thinkingDistinct);
  const [canDisable, setCanDisable] = useState(model.thinkingDisable);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
    setSaving(true);
    setError(null);
    try {
      await onSave({
        providerId: model.providerId, modelName: model.modelName, displayName: name.trim(),
        protocol, contextWindow, outputLimit, thinkingLevels: levels,
        thinkingDistinct: distinct, thinkingDisable: canDisable,
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
      <fieldset className="space-y-3" disabled={saving}>
        <legend className="mb-3 text-sm font-medium">可选思考档位</legend>
        <div className="flex flex-wrap gap-4">
          {LEVELS.map(level => <Label key={level} className="flex items-center gap-2" htmlFor={`model-level-${level}`}>
            <Checkbox id={`model-level-${level}`} checked={levels.includes(level)} disabled={saving} onCheckedChange={checked => {
              setLevels(current => LEVELS.filter(value => value === level ? checked === true : current.includes(value)));
              if (checked !== true) setDistinct(current => current.filter(value => value !== level));
            }} />
            {LEVEL_LABEL[level]}
          </Label>)}
        </div>
      </fieldset>
      <fieldset className="space-y-3" disabled={saving}>
        <legend className="mb-3 text-sm font-medium">实际独立档位</legend>
        <div className="flex flex-wrap gap-4">
          {LEVELS.map(level => <Label key={level} className="flex items-center gap-2" htmlFor={`model-distinct-${level}`}>
            <Checkbox id={`model-distinct-${level}`} checked={distinct.includes(level)} disabled={saving || !levels.includes(level)}
              onCheckedChange={checked => setDistinct(current => LEVELS.filter(value => value === level ? checked === true : current.includes(value)))} />
            {LEVEL_LABEL[level]}
          </Label>)}
        </div>
      </fieldset>
      <Label className="flex items-center gap-2" htmlFor="model-can-disable">
        <Checkbox id="model-can-disable" checked={canDisable} disabled={saving} onCheckedChange={value => setCanDisable(value === true)} />
        支持关闭思考
      </Label>
      <div className="flex justify-end gap-2">
        <Button variant="outline" onClick={onCancel} disabled={saving}>取消</Button>
        <Button type="submit" disabled={saving}>{saving ? "保存中" : "保存"}</Button>
      </div>
    </form>
  );
}
