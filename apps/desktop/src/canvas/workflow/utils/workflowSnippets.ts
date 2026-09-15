import type { NodeConfigSchema, ProfessionalNodeKind, WorkflowEdge, WorkflowNode } from "../types";
import { getNodeDefinition, normalizeProfessionalNode } from "./nodeRegistry";
import { getVariableReferences, variableReference } from "./workflowVariables";

// Local-only format, deliberately separate from runtime nodes and workflow export.
export const SNIPPET_LIMITS = { items: 50, nodes: 100, edges: 500, name: 80, itemChars: 200_000, libraryChars: 2_000_000 };
export const SNIPPET_SAFETY_NOTICE = "保存不含运行结果、密码字段及 Authorization / Proxy-Authorization / Cookie / X-API-Key 请求头；插入后请重新配置。任意工具参数、提示词和请求体仍可能含敏感信息，请先检查；片段仅保存在本机。";
const UNSAFE_KEYS = new Set(["__proto__", "prototype", "constructor"]);
const SECRET_HEADERS = new Set(["authorization", "proxy-authorization", "cookie", "x-api-key"]);
// No group editor exists yet. Preserve explicit ID-list metadata only at node/data level,
// not similarly named values inside arbitrary tool parameters.
const CHILD_KEYS = ["childNodeIds", "childrenIds", "childIds", "children"] as const;
type ChildLinks = Partial<Record<typeof CHILD_KEYS[number], string[]>>;
export interface SnippetNode extends ChildLinks {
  id: string;
  type: string;
  position: { x: number; y: number };
  parentId?: string;
  data: { kind: ProfessionalNodeKind; label: string } & Record<string, unknown>;
}
export interface WorkflowFragment { nodes: SnippetNode[]; edges: WorkflowEdge[] }
export interface WorkflowSnippet extends WorkflowFragment { id: string; name: string; createdAt: number }
export interface SnippetLibrary { version: 1; snippets: WorkflowSnippet[] }

function fail(message: string): never { throw new Error(message); }
function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function own(value: object, key: string): boolean { return Object.prototype.hasOwnProperty.call(value, key); }
function identifier(value: unknown): value is string {
  return typeof value === "string" && /^[\w-]{1,160}$/.test(value) && !UNSAFE_KEYS.has(value);
}
function allowedKeys(value: Record<string, unknown>, keys: string[]): void {
  if (Object.keys(value).some((key) => !keys.includes(key))) fail("片段包含未知字段；请保留原始库并检查版本，未覆盖数据。");
}

// Bounded JSON cloning rejects prototype pollution, cycles, exotic objects and non-finite values.
function cloneJson(value: unknown, depth = 0, budget = { count: 0 }): unknown {
  if (depth > 32 || ++budget.count > 40_000) fail("片段配置嵌套过深或过大，请缩小选区。");
  if (value === null || typeof value === "boolean" || typeof value === "string") return value;
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (Array.isArray(value)) return value.map((item) => cloneJson(item, depth + 1, budget));
  if (!record(value) || ![Object.prototype, null].includes(Object.getPrototypeOf(value))) fail("片段配置必须是可序列化的 JSON 数据。");
  return Object.fromEntries(Object.entries(value).flatMap(([key, entry]) => {
    if (UNSAFE_KEYS.has(key)) fail("片段包含不安全字段，已拒绝读取或保存。");
    return entry === undefined ? [] : [[key, cloneJson(entry, depth + 1, budget)]];
  }));
}

function sanitize(value: unknown, schema?: NodeConfigSchema, headers = false): unknown {
  if (schema?.["x-widget"] === "password") return undefined;
  if (schema?.type) {
    const actual = Array.isArray(value) ? "array" : value === null ? "null" : typeof value;
    if (schema.type === "integer" ? !Number.isInteger(value) : actual !== schema.type) fail("片段配置类型与节点 schema 不一致，请先修正配置。");
  }
  if (Array.isArray(value)) return value.flatMap((item) => {
    const clean = sanitize(item, schema?.items, headers);
    return clean === undefined ? [] : [clean];
  });
  if (!record(value)) return value;
  return Object.fromEntries(Object.entries(value).flatMap(([key, entry]) => {
    if (headers && SECRET_HEADERS.has(key.trim().toLowerCase())) return [];
    const declared = schema?.properties && own(schema.properties, key);
    if (schema?.properties && !declared && !schema.additionalProperties) return [];
    const child = declared ? schema!.properties![key] : typeof schema?.additionalProperties === "object" ? schema.additionalProperties : undefined;
    const clean = sanitize(entry, child, /^(httpHeaders|webhookHeaders|headers)$/i.test(key));
    return clean === undefined ? [] : [[key, clean]];
  }));
}

function copyLinks(source: Record<string, unknown>): ChildLinks {
  return Object.fromEntries(CHILD_KEYS.flatMap((key) => {
    if (source[key] === undefined) return [];
    const ids = source[key];
    if (!Array.isArray(ids) || !ids.every(identifier) || new Set(ids).size !== ids.length) fail("分组子节点引用无效，请检查完整分组选区。");
    return [[key, [...ids]]];
  }));
}

function captureNode(node: WorkflowNode): SnippetNode {
  if (!identifier(node.id) || !node.type?.startsWith("professional-")) fail("请选择可识别的专业工作流节点。");
  const normalized = normalizeProfessionalNode(node);
  const definition = getNodeDefinition(normalized.data.kind as ProfessionalNodeKind);
  if (node.type !== `professional-${definition.kind}`) fail("节点类型与配置不一致，请先修正节点。");
  const source = normalized.data;
  const picked = Object.fromEntries(Object.keys(definition.configSchema.properties ?? {}).flatMap((key) =>
    source[key] === undefined ? [] : [[key, source[key]]],
  ));
  const config = sanitize(cloneJson(picked), definition.configSchema) as Record<string, unknown>;
  if (typeof source.label !== "string") fail("片段节点名称无效。");
  return {
    id: node.id, type: node.type, position: { ...node.position },
    ...(node.parentId !== undefined ? { parentId: node.parentId } : {}),
    ...copyLinks(node as unknown as Record<string, unknown>),
    data: { ...config, ...copyLinks(source), kind: definition.kind, label: source.label },
  };
}

function mapReferences(value: unknown, ids: Map<string, string>): unknown {
  if (typeof value === "string") {
    const replacements = new Map(getVariableReferences(value).map(([id, ...fields]) => {
      const mapped = ids.get(id);
      if (!mapped) fail(`片段引用了未选中的节点 ${id}；请同时选择该节点，或移除对应变量引用。`);
      return [variableReference(id, fields.join(".")), variableReference(mapped, fields.join("."))];
    }));
    // Tokenize first: replacements cannot cascade or alter noncanonical text / ID substrings.
    return value.split(/(\{\{#[^{}]*#\}\})/g).map((part) => replacements.get(part) ?? part).join("");
  }
  if (Array.isArray(value)) return value.map((item) => mapReferences(item, ids));
  if (record(value)) return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, mapReferences(entry, ids)]));
  return value;
}

function mapLinks(source: Record<string, unknown>, ids: Map<string, string>): ChildLinks {
  return Object.fromEntries(Object.entries(copyLinks(source)).map(([key, children]) => [key, children.map((id) => {
    const mapped = ids.get(id);
    if (!mapped) fail(`分组引用了未选中的子节点 ${id}；请完整选择分组及其所有子节点。`);
    return mapped;
  })]));
}

function validateGraph(fragment: WorkflowFragment): void {
  const { nodes, edges } = fragment;
  if (!nodes.length || nodes.length > SNIPPET_LIMITS.nodes || edges.length > SNIPPET_LIMITS.edges) fail("片段需包含 1–100 个节点，且不超过 500 条内部连线。");
  const ids = new Map(nodes.map((node) => [node.id, node.id]));
  if (ids.size !== nodes.length) fail("片段节点 ID 重复。");
  const adjacency = new Map(nodes.map((node) => [node.id, new Set<string>()]));
  const link = (a: string, b: string) => {
    if (!ids.has(a) || !ids.has(b)) fail("片段分组或连线引用了未选中的节点；请完整选择关联节点。");
    adjacency.get(a)!.add(b); adjacency.get(b)!.add(a);
  };
  const parents = new Map(nodes.map((node) => [node.id, node.parentId]));
  for (const node of nodes) {
    if (!identifier(node.id) || !Number.isFinite(node.position.x) || !Number.isFinite(node.position.y)) fail("片段节点 ID 或坐标无效。");
    mapReferences(node.data, ids);
    for (const source of [node as unknown as Record<string, unknown>, node.data]) {
      for (const children of Object.values(mapLinks(source, ids))) for (const child of children) link(node.id, child);
    }
    const visited = new Set([node.id]);
    let parent = node.parentId;
    while (parent !== undefined) {
      if (!identifier(parent) || visited.has(parent)) fail("分组父节点引用无效或形成循环，请先修正分组。");
      link(node.id, parent); visited.add(parent); parent = parents.get(parent);
    }
  }
  const edgeIds = new Set<string>();
  for (const edge of edges) {
    if (!identifier(edge.id) || edgeIds.has(edge.id)) fail("片段连线 ID 无效或重复。");
    edgeIds.add(edge.id); link(edge.source, edge.target);
    for (const handle of [edge.sourceHandle, edge.targetHandle]) {
      if (handle !== undefined && handle !== null && (typeof handle !== "string" || handle.length > 200)) fail("片段连线端口无效。");
    }
  }
  const reached = new Set<string>();
  const pending = [nodes[0].id];
  while (pending.length) {
    const id = pending.pop()!;
    if (reached.has(id)) continue;
    reached.add(id); pending.push(...adjacency.get(id)!);
  }
  if (reached.size !== nodes.length) fail("选区不连通；请选择通过内部连线或分组连接的节点后保存。");
}

export function captureWorkflowFragment(nodes: WorkflowNode[], edges: WorkflowEdge[]): WorkflowFragment {
  const selected = nodes.filter((node) => node.selected && node.type?.startsWith("professional-"));
  if (!selected.length) fail("请先在专业画布中选择要保存的节点。");
  const ids = new Set(selected.map((node) => node.id));
  // Selecting a group without all its children is not a self-contained fragment.
  for (const node of nodes) {
    if (node.parentId && ids.has(node.parentId) && !ids.has(node.id)) fail("请完整选择分组及其所有子节点后保存。");
    if (!ids.has(node.id)) {
      for (const source of [node as unknown as Record<string, unknown>, node.data]) {
        if (Object.values(copyLinks(source)).some((children) => children.some((id) => ids.has(id)))) fail("请同时选择子节点所属分组。");
      }
    }
  }
  const captured = selected.map(captureNode);
  const roots = captured.filter((node) => !node.parentId);
  const origin = { x: Math.min(...roots.map((node) => node.position.x)), y: Math.min(...roots.map((node) => node.position.y)) };
  const fragment: WorkflowFragment = {
    nodes: captured.map((node) => node.parentId ? node : { ...node, position: { x: node.position.x - origin.x, y: node.position.y - origin.y } }),
    edges: edges.filter((edge) => ids.has(edge.source) && ids.has(edge.target)).map((edge) => ({
      id: edge.id, source: edge.source, target: edge.target,
      ...(edge.sourceHandle !== undefined ? { sourceHandle: edge.sourceHandle } : {}),
      ...(edge.targetHandle !== undefined ? { targetHandle: edge.targetHandle } : {}),
    })),
  };
  validateGraph(fragment);
  if (JSON.stringify(fragment).length > SNIPPET_LIMITS.itemChars) fail("片段过大，请缩小选区或配置内容。");
  return fragment;
}

export function validateFragment(value: unknown): WorkflowFragment {
  const clean = cloneJson(value);
  if (!record(clean)) fail("片段结构无效。");
  allowedKeys(clean, ["nodes", "edges"]);
  if (!Array.isArray(clean.nodes) || !Array.isArray(clean.edges)) fail("片段缺少节点或连线数组。");
  for (const node of clean.nodes) {
    if (!record(node) || !record(node.data) || !record(node.position)) fail("片段节点结构无效。");
    allowedKeys(node, ["id", "type", "position", "data", "parentId", ...CHILD_KEYS]);
    allowedKeys(node.position, ["x", "y"]);
    if (!identifier(node.id) || typeof node.type !== "string" || typeof node.data.kind !== "string" || typeof node.data.label !== "string") fail("片段节点类型或名称无效。");
    const definition = getNodeDefinition(node.data.kind as ProfessionalNodeKind);
    if (node.type !== `professional-${definition.kind}`) fail("片段节点类型与配置不一致。");
    allowedKeys(node.data, ["kind", "label", ...Object.keys(definition.configSchema.properties ?? {}), ...CHILD_KEYS]);
    const config = Object.fromEntries(Object.entries(node.data).filter(([key]) => key !== "kind" && key !== "label" && !CHILD_KEYS.includes(key as typeof CHILD_KEYS[number])));
    if (JSON.stringify(sanitize(config, definition.configSchema)) !== JSON.stringify(config)) fail("存储的片段含敏感或未知配置；已保留原数据并拒绝加载。");
    copyLinks(node); copyLinks(node.data);
  }
  for (const edge of clean.edges) {
    if (!record(edge)) fail("片段连线结构无效。");
    allowedKeys(edge, ["id", "source", "target", "sourceHandle", "targetHandle"]);
  }
  const fragment = clean as unknown as WorkflowFragment;
  validateGraph(fragment);
  if (JSON.stringify(fragment).length > SNIPPET_LIMITS.itemChars) fail("片段超出大小限制。");
  return fragment;
}

export function parseSnippetLibrary(raw: string | null): SnippetLibrary {
  if (raw === null) return { version: 1, snippets: [] };
  if (raw.length > SNIPPET_LIMITS.libraryChars) fail("本地片段库超出大小限制，未修改原始数据。");
  let value: unknown;
  try { value = JSON.parse(raw); } catch { fail("本地片段库 JSON 损坏，请保留原始数据并修复后重试。"); }
  if (!record(value)) fail("本地片段库结构无效。");
  allowedKeys(value, ["version", "snippets"]);
  if (value.version !== 1 || !Array.isArray(value.snippets)) fail("不支持此片段库版本或结构，未覆盖原始数据。");
  if (value.snippets.length > SNIPPET_LIMITS.items) fail("片段库最多保存 50 项。");
  const ids = new Set<string>(); const names = new Set<string>();
  const snippets = value.snippets.map((item: unknown): WorkflowSnippet => {
    if (!record(item)) fail("本地片段条目无效。");
    allowedKeys(item, ["id", "name", "createdAt", "nodes", "edges"]);
    if (!identifier(item.id) || ids.has(item.id)) fail("片段 ID 无效或重复。");
    const name = validateSnippetName(item.name);
    if (name !== item.name || names.has(name.toLowerCase())) fail("片段名称无效或重复。");
    if (typeof item.createdAt !== "number" || !Number.isFinite(item.createdAt) || item.createdAt < 0) fail("片段创建时间无效。");
    ids.add(item.id); names.add(name.toLowerCase());
    return { id: item.id, name, createdAt: item.createdAt, ...validateFragment({ nodes: item.nodes, edges: item.edges }) };
  });
  return { version: 1, snippets };
}

export function validateSnippetName(value: unknown): string {
  if (typeof value !== "string" || !value.trim() || value.trim().length > SNIPPET_LIMITS.name) fail("请输入 1–80 个字符的片段名称。");
  return value.trim();
}

export function instantiateWorkflowFragment(
  value: WorkflowFragment, x: number, y: number, existingNodes: WorkflowNode[], existingEdges: WorkflowEdge[],
): { nodes: WorkflowNode[]; edges: WorkflowEdge[] } {
  const fragment = validateFragment(value);
  if (!Number.isFinite(x) || !Number.isFinite(y)) fail("插入位置无效，请重新打开节点选择器。");
  const taken = new Set([...existingNodes, ...existingEdges, ...fragment.nodes, ...fragment.edges].map((item) => item.id));
  const mint = (prefix: string) => {
    const base = `${prefix}-${crypto.randomUUID()}`;
    let id = base; let suffix = 0;
    while (taken.has(id)) id = `${base}-${++suffix}`;
    taken.add(id); return id;
  };
  const ids = new Map(fragment.nodes.map((node) => [node.id, mint("pro-snippet")]));
  // Parents precede their children as required by React Flow, without changing graph topology.
  const ordered: SnippetNode[] = [];
  const byId = new Map(fragment.nodes.map((node) => [node.id, node]));
  const emitted = new Set<string>();
  const emit = (node: SnippetNode) => {
    if (emitted.has(node.id)) return;
    if (node.parentId) emit(byId.get(node.parentId)!);
    emitted.add(node.id); ordered.push(node);
  };
  fragment.nodes.forEach(emit);
  return {
    nodes: ordered.map((node) => normalizeProfessionalNode({
      ...node, ...mapLinks(node as unknown as Record<string, unknown>, ids), id: ids.get(node.id)!,
      ...(node.parentId ? { parentId: ids.get(node.parentId)! } : {}),
      position: node.parentId ? { ...node.position } : { x: x + node.position.x, y: y + node.position.y },
      data: { ...mapReferences(node.data, ids) as SnippetNode["data"], ...mapLinks(node.data, ids), status: "idle" },
      selected: true,
    })),
    edges: fragment.edges.map((edge) => ({ ...edge, id: mint("pro-snippet-edge"), source: ids.get(edge.source)!, target: ids.get(edge.target)! })),
  };
}
