import { useEffect, useRef, useState } from "react";
import { Plus, X } from "lucide-react";
import { Button } from "../../../components/shadcn/button";
import type { NodeConfigSchema, ProfessionalNodeData } from "../types";
import { getNodeConfigIssues, getNodeDefinition } from "../utils/nodeRegistry";
import { createSchemaValue, reconcileArrayKeys, validateSchemaValue } from "../utils/configSchema";
import { useCanvasSettingsStore } from "../store/canvasSettingsStore";
import { CANVAS_BUTTON_CLASS, CanvasField, CanvasInput, CanvasSelect, CanvasTextarea } from "./CanvasFields";
import { VariablePicker } from "./VariablePicker";
import { NodeOutputPanel } from "./NodeOutputPanel";

type EditorProps = {
  nodeId: string;
  path: string;
  schema: NodeConfigSchema;
  value: unknown;
  onChange: (value: unknown) => void;
};

function JsonEditor({ value, onChange, schema, path }: EditorProps) {
  const serialized = value === undefined ? "" : JSON.stringify(value, null, 2);
  const [draft, setDraft] = useState(serialized);
  const [error, setError] = useState("");
  useEffect(() => { setDraft(serialized); setError(""); }, [serialized]);
  return <>
    <CanvasTextarea id={path} aria-label={schema.title} aria-invalid={!!error} className="font-mono" value={draft} placeholder="JSON" onChange={(event) => {
      const next = event.target.value;
      setDraft(next);
      if (!next.trim()) { setError(""); onChange(undefined); return; }
      try {
        const parsed: unknown = JSON.parse(next);
        const issue = validateSchemaValue(schema, parsed)[0];
        if (issue) { setError(issue.message); return; }
        setError(""); onChange(parsed);
      } catch { setError("无效 JSON，尚未保存"); }
    }} />
    {error && <p role="alert" className="text-[11px] text-red-300">{error}</p>}
  </>;
}

function ModelEditor({ schema, value, onChange, path }: EditorProps) {
  const providers = useCanvasSettingsStore((state) => state.providers);
  const options = providers.filter((provider) => provider.enabled !== false).flatMap((provider) =>
    (provider.models ?? []).filter((model) => model.enabled && model.scenarios?.includes("text")).map((model) => ({
      value: `${provider.id}::${model.id}`, label: `${provider.name} / ${model.name}`,
    })),
  );
  return <CanvasSelect id={path} label={schema.title} value={String(value ?? "")} onChange={onChange} options={[{ value: "", label: "使用默认文本模型" }, ...options]} />;
}

function ArrayEditor(props: EditorProps) {
  const { schema, value, path, onChange } = props;
  const entries = Array.isArray(value) ? value : [];
  const identity = useRef<{ entries: unknown[]; keys: string[] }>({ entries: [], keys: [] });
  if (identity.current.entries !== entries) {
    identity.current = { entries, keys: reconcileArrayKeys(identity.current.entries, identity.current.keys, entries) };
  }
  const keys = identity.current.keys;
  const update = (next: unknown[], nextKeys: string[]) => {
    identity.current = { entries: next, keys: nextKeys };
    onChange(next);
  };
  return (
    <div className="flex flex-col gap-2">
      {entries.map((entry, index) => (
        <div key={keys[index]} className="flex min-w-0 items-start gap-1 border-l border-white/10 pl-2">
          <div className="min-w-0 flex-1">
            <SchemaEditor {...props} schema={schema.items ?? {}} value={entry} path={`${path}.${index}`}
              onChange={(next) => update(entries.map((current, i) => i === index ? next : current), keys)} />
          </div>
          <Button size="icon" variant="ghost" className={`${CANVAS_BUTTON_CLASS} !h-7 !w-7`}
            aria-label={`移除${schema.title ?? "项目"} ${index + 1}`}
            onClick={() => update(entries.filter((_, i) => i !== index), keys.filter((_, i) => i !== index))}>
            <X className="h-3 w-3" />
          </Button>
        </div>
      ))}
      <Button variant="ghost" size="sm" className={`${CANVAS_BUTTON_CLASS} self-start`}
        onClick={() => update([...entries, createSchemaValue(schema.items ?? {})], [...keys, crypto.randomUUID()])}>
        <Plus className="h-3 w-3" />添加{schema.title ?? "项目"}
      </Button>
    </div>
  );
}

function SchemaEditor(props: EditorProps) {
  const { nodeId, path, schema, value, onChange } = props;
  const widget = schema["x-widget"];
  if (widget === "json") return <JsonEditor {...props} />;
  if (widget === "model") return <ModelEditor {...props} />;
  if (schema.type === "object" && schema.properties) {
    return <SchemaObjectEditor {...props} />;
  }
  if (schema.type === "object" || !schema.type) return <JsonEditor {...props} />;
  if (schema.type === "array") return <ArrayEditor {...props} />;
  if (schema.enum || schema.type === "boolean") {
    const values = schema.enum ?? [true, false];
    return <CanvasSelect id={path} label={schema.title} disabled={schema.readOnly} value={String(value ?? "")} options={values.map((entry, index) => ({ value: String(entry), label: schema["x-enum-labels"]?.[index] ?? (typeof entry === "boolean" ? entry ? "是" : "否" : String(entry)) }))} onChange={(next) => onChange(values.find((entry) => String(entry) === next))} />;
  }
  if (widget === "textarea" || widget === "code") {
    return <div className="flex flex-col gap-1.5"><CanvasTextarea id={path} aria-label={schema.title} value={String(value ?? "")} className={widget === "code" ? "font-mono" : ""} onChange={(event) => onChange(event.target.value)} />{widget !== "code" && <div className="self-start"><VariablePicker nodeId={nodeId} label={`插入${schema.title ?? "文本"}变量`} onSelect={(reference) => onChange(`${value ?? ""}${reference}`)} /></div>}</div>;
  }
  if (widget === "variable") {
    return <div className="flex min-w-0 items-start gap-1"><CanvasInput id={path} aria-label={schema.title} className="min-w-0 flex-1 font-mono" value={String(value ?? "")} onChange={(event) => onChange(event.target.value)} placeholder="输入值或引用上游变量" /><VariablePicker nodeId={nodeId} types={schema["x-variable-types"]} label={`选择${schema.title ?? ""}变量`} onSelect={onChange} /></div>;
  }
  const numeric = schema.type === "number" || schema.type === "integer";
  return <CanvasInput id={path} aria-label={schema.title} readOnly={schema.readOnly} type={numeric ? "number" : widget === "password" ? "password" : "text"} min={schema.minimum} max={schema.maximum} step={schema.type === "integer" ? 1 : "any"} value={typeof value === "number" || typeof value === "string" ? value : ""} onChange={(event) => onChange(numeric ? event.target.value === "" ? undefined : Number(event.target.value) : event.target.value)} />;
}

export function SchemaObjectEditor(props: EditorProps) {
  const { schema, value, path, onChange } = props;
  const record = value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
  return <div className="flex min-w-0 flex-col gap-3">
    {Object.entries(schema.properties ?? {}).map(([key, child]) => {
      const condition = child["x-visible-when"];
      if (condition && record[condition.field] !== condition.value) return null;
      const childPath = `${path}.${key}`;
      return <CanvasField key={key} htmlFor={childPath} label={`${child.title ?? key}${schema.required?.includes(key) ? " *" : ""}`}>
        <SchemaEditor {...props} path={childPath} schema={child} value={record[key]} onChange={(next) => onChange({ ...record, [key]: next })} />
        {child.description && <p className="text-[10px] leading-relaxed text-white/35">{child.description}</p>}
      </CanvasField>;
    })}
  </div>;
}

export function SchemaConfigForm({ nodeId, data, onUpdate }: { nodeId: string; data: ProfessionalNodeData; onUpdate: (patch: Record<string, unknown>) => void }) {
  const definition = getNodeDefinition(data.kind);
  const issues = getNodeConfigIssues(data.kind, data);
  return <div className="nodrag nopan nowheel max-h-[65vh] overflow-y-auto pr-1" onPointerDown={(event) => event.stopPropagation()} onDoubleClick={(event) => event.stopPropagation()} onKeyDown={(event) => event.stopPropagation()}>
    {definition.availability === "reserved" && <p className="mb-3 text-[11px] text-amber-200/80">该节点的外部连接尚未启用，配置不会自动启动服务。</p>}
    <SchemaObjectEditor nodeId={nodeId} path={nodeId} schema={definition.configSchema} value={data} onChange={(next) => onUpdate(Object.fromEntries(Object.entries(next as Record<string, unknown>).filter(([key, value]) => !Object.is(value, data[key]))))} />
    {!!issues.length && <div role="status" className="mt-3 text-[11px] text-amber-200/80">{issues.map((issue) => <p key={`${issue.path}:${issue.message}`}>{issue.path}：{issue.message}</p>)}</div>}
    <NodeOutputPanel nodeId={nodeId} data={data} />
  </div>;
}
