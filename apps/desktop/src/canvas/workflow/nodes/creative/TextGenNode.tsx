import type { CreativeNodeData } from "../../types";
import { NodeTextOutput } from "../../components/NodeTextOutput";

export function TextGenContent({ data }: { data: CreativeNodeData }) {
  return <NodeTextOutput output={data.output} status={data.status} />;
}
