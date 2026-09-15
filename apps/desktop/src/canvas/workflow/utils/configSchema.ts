import type { NodeConfigSchema } from "../types";

export interface ConfigIssue { path: string; message: string }

export function reconcileArrayKeys(previous: unknown[], keys: string[], next: unknown[]): string[] {
  const claimed = new Set<number>();
  return next.map((value) => {
    const index = previous.findIndex((entry, i) => !claimed.has(i) && Object.is(entry, value));
    if (index < 0) return crypto.randomUUID();
    claimed.add(index);
    return keys[index];
  });
}

export function createSchemaValue(schema: NodeConfigSchema): unknown {
  if (schema.default !== undefined) return structuredClone(schema.default);
  if (schema.format === "uuid") return crypto.randomUUID();
  if (schema.enum?.length) return schema.enum[0];
  if (schema.type === "object") {
    return Object.fromEntries(Object.entries(schema.properties ?? {}).flatMap(([key, child]) =>
      child.default !== undefined || child.format === "uuid" || schema.required?.includes(key)
        ? [[key, createSchemaValue(child)]] : [],
    ));
  }
  if (schema.type === "array") return [];
  if (schema.type === "boolean") return false;
  if (schema.type === "number" || schema.type === "integer") return schema.minimum ?? 0;
  return "";
}

export function validateSchemaValue(schema: NodeConfigSchema, value: unknown, path = ""): ConfigIssue[] {
  if (value === undefined) return [];
  const issue = (message: string): ConfigIssue[] => [{ path, message }];
  const actualType = Array.isArray(value) ? "array" : value === null ? "null" : typeof value;
  if (schema.type && !(schema.type === "integer" ? Number.isInteger(value) : actualType === schema.type)) {
    return issue(`需要 ${schema.type} 类型`);
  }
  if (schema.enum && !schema.enum.some((entry) => entry === value)) return issue("请选择有效选项");
  if (typeof value === "number") {
    if (!Number.isFinite(value)) return issue("请输入有限数字");
    if (schema.minimum !== undefined && value < schema.minimum) return issue(`不能小于 ${schema.minimum}`);
    if (schema.maximum !== undefined && value > schema.maximum) return issue(`不能大于 ${schema.maximum}`);
  }
  if (typeof value === "string") {
    if (schema.minLength !== undefined && value.length < schema.minLength) return issue(`至少 ${schema.minLength} 个字符`);
    if (schema.pattern && !new RegExp(schema.pattern).test(value)) return issue("格式不正确");
  }
  if (Array.isArray(value)) {
    const issues = schema.minItems !== undefined && value.length < schema.minItems ? issue(`至少 ${schema.minItems} 项`) : [];
    const names = new Set<string>();
    value.forEach((entry, index) => {
      if (entry && typeof entry === "object" && typeof entry.name === "string" && entry.name) {
        if (names.has(entry.name)) issues.push({ path: `${path}.${index}.name`, message: "名称不能重复" });
        names.add(entry.name);
      }
      if (schema.items) issues.push(...validateSchemaValue(schema.items, entry, `${path}.${index}`));
    });
    return issues;
  }
  if (value !== null && typeof value === "object") {
    const record = value as Record<string, unknown>;
    const issues: ConfigIssue[] = [];
    for (const key of schema.required ?? []) {
      if (!Object.prototype.hasOwnProperty.call(record, key) || record[key] === undefined) {
        issues.push({ path: path ? `${path}.${key}` : key, message: "此项必填" });
      }
    }
    for (const [key, child] of Object.entries(schema.properties ?? {})) {
      const condition = child["x-visible-when"];
      if (condition && record[condition.field] !== condition.value) continue;
      issues.push(...validateSchemaValue(child, record[key], path ? `${path}.${key}` : key));
    }
    for (const [key, entry] of Object.entries(record)) {
      if (Object.prototype.hasOwnProperty.call(schema.properties ?? {}, key)) continue;
      const childPath = path ? `${path}.${key}` : key;
      if (schema.additionalProperties === false) issues.push({ path: childPath, message: "不支持的字段" });
      else if (typeof schema.additionalProperties === "object") issues.push(...validateSchemaValue(schema.additionalProperties, entry, childPath));
    }
    return issues;
  }
  return [];
}
