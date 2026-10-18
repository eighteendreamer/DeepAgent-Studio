import type { CreativeNodeData } from "../../types";
import { NodeTextOutput } from "../../components/NodeTextOutput";

export function ScriptGenContent({ data }: { data: CreativeNodeData }) {
  return <NodeTextOutput output={data.output} status={data.status} monospace />;
}
