import { WorkingOrb } from "./WorkingOrb";
import { formatMs } from "./format";
import { CollapseChevron } from "./CollapseBlurPanel";
import { Button } from "../shadcn/button";

export function WorkMetaRow({
  processing,
  totalMs,
  expanded,
  onToggle,
}: {
  processing: boolean;
  totalMs: number;
  expanded: boolean;
  onToggle: () => void;
}) {
  return (
    <Button
      variant="ghost"
      type="button"
      data-process-toggle
      aria-expanded={expanded}
      onClick={onToggle}
      className="group h-auto w-fit max-w-full justify-start gap-1.5 rounded-md px-0 py-0.5 text-left text-[14px] font-medium text-text-secondary hover:bg-transparent hover:text-text-base"
    >
      <span className="flex h-5 w-5 items-center justify-center">
        <WorkingOrb active={processing} />
      </span>
      <span>深度探索</span>
      {totalMs > 0 && <span className="text-[12px] font-normal text-text-secondary">{formatMs(totalMs)}</span>}
      <CollapseChevron open={expanded} className="opacity-45 group-hover:opacity-80" />
    </Button>
  );
}
