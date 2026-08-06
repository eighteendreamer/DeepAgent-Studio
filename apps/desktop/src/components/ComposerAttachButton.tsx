import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { cn } from "./shadcn/utils";

type Props = {
  disabled?: boolean;
  disabledTitle?: string;
  onClick: () => void;
};

/** Composer「+」—— 悬停 morph 展开（B 弹簧 + C 图标旋转），点击添加附件 */
export function ComposerAttachButton({ disabled = false, disabledTitle, onClick }: Props) {
  const { t } = useTranslation();

  return (
    <button
      type="button"
      disabled={disabled}
      title={disabled ? disabledTitle : undefined}
      aria-label={t("composer.addMenu.hoverLabel")}
      onClick={onClick}
      className={cn(
        "group/add flex h-8 w-8 shrink-0 items-center overflow-hidden rounded-full bg-black/5 text-text-secondary",
        "transition-[width,background-color,padding] duration-500 ease-out motion-reduce:transition-none",
        !disabled && "hover:w-[7.25rem] hover:pr-2.5 hover:bg-black/[0.08] hover:text-text-base",
        disabled && "cursor-not-allowed opacity-40",
      )}
    >
      <span
        className={cn(
          "flex h-8 w-8 shrink-0 items-center justify-center",
          "transition-transform duration-500 ease-out motion-reduce:transition-none",
          !disabled && "group-hover/add:rotate-90",
        )}
      >
        <FontAwesomeIcon icon={["fas", "plus"]} className="text-[11px]" />
      </span>
      <span
        className={cn(
          "overflow-hidden whitespace-nowrap text-[12px] font-medium text-text-base",
          "max-w-0 opacity-0 transition-[max-width,opacity] duration-500 ease-out motion-reduce:transition-none",
          !disabled &&
            "group-hover/add:max-w-[4.5rem] group-hover/add:opacity-100 group-hover/add:delay-100",
        )}
      >
        {t("composer.addMenu.hoverLabel")}
      </span>
    </button>
  );
}
