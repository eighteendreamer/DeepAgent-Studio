import { useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { ChatBlock, ProcessSection } from "./timelineTypes";
import { MarkdownText } from "../MarkdownText";
import { ProcessToolRow } from "./ProcessToolRow";

function toolLabel(block: Extract<ChatBlock, { kind: "tool" }>): string {
  const tool = block.tool;
  const raw = tool.detail?.trim();
  if (raw) return raw.length > 96 ? `${raw.slice(0, 95).trimEnd()}…` : raw;
  if (tool.args?.trim()) return tool.name;
  return tool.name || "tool";
}

function sectionTitle(section: ProcessSection): string {
  if (section.kind === "reasoning") return "思考";
  if (section.kind === "output") return "中间输出";
  if (section.blocks.length === 1 && section.blocks[0]?.kind === "tool") {
    return toolLabel(section.blocks[0]);
  }
  return `执行 ${section.blocks.length} 步`;
}

function sectionIcon(section: ProcessSection): "lightbulb" | "terminal" | "robot" {
  if (section.kind === "reasoning") return "lightbulb";
  if (section.kind === "output") return "robot";
  return "terminal";
}

function blockHasError(block: ChatBlock): boolean {
  return block.kind === "tool" && (block.tool.status === "error" || block.tool.status === "blocked");
}

function ProcessBlockDetail({ block }: { block: ChatBlock }) {
  if (block.kind === "reasoning") {
    return <MarkdownText text={block.text} className="text-[13.5px] text-text-secondary" />;
  }
  if (block.kind === "assistant") {
    return <MarkdownText text={block.text} tone={block.tone} className="text-[13.5px]" />;
  }
  if (block.kind === "tool") {
    return <ProcessToolRow tool={block.tool} />;
  }
  return null;
}

export function ProcessSectionRow({
  section,
  processing,
}: {
  section: ProcessSection;
  processing: boolean;
}) {
  const hasError = section.blocks.some(blockHasError);
  const active = processing && section.blocks.some((block) => block.kind === "tool" && block.tool.status === "running");
  const defaultOpen = (processing && (section.kind === "reasoning" || hasError)) || hasError;
  const [userOpen, setUserOpen] = useState<boolean | null>(null);
  const open = userOpen ?? defaultOpen;
  const canOpen = section.blocks.length > 0;
  const title = useMemo(() => sectionTitle(section), [section]);
  const icon = sectionIcon(section);

  return (
    <div className="min-w-0">
      <button
        type="button"
        onClick={() => canOpen && setUserOpen((value) => !(value ?? defaultOpen))}
        className={`group flex w-full min-w-0 items-center gap-1.5 rounded-md px-1 py-0.5 text-left text-[13.5px] font-medium leading-6 transition ${
          hasError ? "text-orange-700" : "text-text-secondary hover:bg-black/5 hover:text-text-base"
        }`}
      >
        <FontAwesomeIcon icon={["fas", icon]} className="w-4 shrink-0 text-[12px] opacity-75" />
        <span className={`min-w-0 flex-1 truncate ${active && !hasError ? "text-primary" : ""}`}>{title}</span>
        <FontAwesomeIcon
          icon={["fas", open ? "chevron-down" : "chevron-right"]}
          className="shrink-0 text-[10px] opacity-40 transition group-hover:opacity-70"
        />
      </button>
      {open && (
        <div className="ml-5 mt-1 flex min-w-0 flex-col gap-0.5 border-l border-border-theme/80 pl-3">
          {section.blocks.map((block) => (
            <ProcessBlockDetail key={block.id} block={block} />
          ))}
        </div>
      )}
    </div>
  );
}
