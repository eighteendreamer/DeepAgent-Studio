import type { NodeOutput, ProfessionalNodeKind, WorkflowEdge, WorkflowNode } from "../types";
import { getAllNodeDefinitions, getNodeOutputs, normalizeProfessionalData } from "./nodeRegistry";

export interface AvailableVariable extends NodeOutput {
  nodeId: string;
  nodeLabel: string;
  reference: string;
}

const REFERENCE_PATTERN = /\{\{#([\w-]+(?:\.[\w-]+)+)#\}\}/g;

export function variableReference(nodeId: string, name: string): string {
  return `{{#${nodeId}.${name}#}}`;
}

export function getUpstreamNodeIds(nodeId: string, edges: WorkflowEdge[]): Set<string> {
  const incoming = new Map<string, string[]>();
  for (const edge of edges) incoming.set(edge.target, [...(incoming.get(edge.target) ?? []), edge.source]);
  const visited = new Set([nodeId]);
  const pending = [...(incoming.get(nodeId) ?? [])];
  while (pending.length) {
    const id = pending.pop()!;
    if (visited.has(id)) continue;
    visited.add(id);
    pending.push(...(incoming.get(id) ?? []));
  }
  visited.delete(nodeId);
  return visited;
}

export function getAvailableVariables(nodeId: string, nodes: WorkflowNode[], edges: WorkflowEdge[], types?: string[]): AvailableVariable[] {
  const upstream = getUpstreamNodeIds(nodeId, edges);
  return nodes.filter((node) => upstream.has(node.id) && node.type?.startsWith("professional-")).flatMap((node) => {
    const data = normalizeProfessionalData(node.data, node.type);
    if (!getAllNodeDefinitions().some((definition) => definition.kind === data.kind)) return [];
    return getNodeOutputs(data.kind as ProfessionalNodeKind, data)
      .filter((output) => !types?.length || types.includes(output.type))
      .map((output) => ({ ...output, nodeId: node.id, nodeLabel: data.label, reference: variableReference(node.id, output.name) }));
  });
}

export function getVariableReferences(text: string): string[][] {
  return Array.from(text.matchAll(REFERENCE_PATTERN), (match) => match[1].split("."));
}
