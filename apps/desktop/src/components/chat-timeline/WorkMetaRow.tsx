import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { formatMs } from "./format";

export function WorkMetaRow({
  processing,
  stepCount,
  toolCount,
  totalMs,
  expanded,
  onToggle,
}: {
  processing: boolean;
  stepCount: number;
  toolCount: number;
  totalMs: number;
  expanded: boolean;
  onToggle: () => void;
}) {
  const label = processing
    ? toolCount > 0
      ? `正在执行 · ${toolCount} 次工具调用`
      : "正在思考"
    : toolCount > 0
    ? `执行过程 · ${toolCount} 次工具调用`
    : `执行过程 · ${stepCount} 步`;

  return (
    <button
      type="button"
      onClick={onToggle}
      className="group flex w-fit max-w-full items-center gap-1.5 rounded-md py-0.5 text-left text-[14px] font-medium text-text-secondary transition hover:text-text-base"
    >
      <span className="flex h-5 w-5 items-center justify-center">
        {processing ? (
          <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin text-[12px] text-primary" />
        ) : (
          <FontAwesomeIcon icon={["fas", "list-check"]} className="text-[12px]" />
        )}
      </span>
      <span className={processing ? "bg-gradient-to-r from-primary via-blue-500 to-primary bg-[length:200%_100%] bg-clip-text text-transparent animate-pulse" : ""}>
        {label}
      </span>
      {totalMs > 0 && <span className="text-[12px] font-normal text-text-secondary">{formatMs(totalMs)}</span>}
      <FontAwesomeIcon
        icon={["fas", expanded ? "chevron-down" : "chevron-right"]}
        className="text-[10px] opacity-45 transition group-hover:opacity-80"
      />
    </button>
  );
}
