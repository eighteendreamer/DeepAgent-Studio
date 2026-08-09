import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faThumbtack } from "@fortawesome/free-solid-svg-icons";
import { cn } from "../shadcn/utils";

type Props = {
  pinned: boolean;
  className?: string;
};

const [FA_W, FA_H, , , FA_PATH] = faThumbtack.icon as [number, number, unknown, unknown, string];

/** 置顶图钉 —— 始终 Font Awesome thumbtack 造型；置顶实心，未置顶同 path 描边 */
export function PinThumbtackIcon({ pinned, className }: Props) {
  if (pinned) {
    return (
      <FontAwesomeIcon
        icon={faThumbtack}
        className={cn("text-[10px] text-text-base", className)}
      />
    );
  }

  return (
    <svg
      viewBox={`0 0 ${FA_W} ${FA_H}`}
      className={cn("h-[1em] w-[0.75em] shrink-0 text-[10px] text-text-secondary", className)}
      aria-hidden
    >
      <path
        fill="none"
        stroke="currentColor"
        strokeWidth="26"
        strokeLinejoin="round"
        strokeLinecap="round"
        d={FA_PATH}
      />
    </svg>
  );
}
