import { HoverInfo } from "./ui/HoverInfo";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { cn } from "./shadcn/utils";
import { Button } from "./shadcn/button";

type Props = {
  disabled?: boolean;
  disabledTitle?: string;
  onClick: () => void;
};

/** Composer attachment entry with a small press response. */
export function ComposerAttachButton({ disabled = false, disabledTitle, onClick }: Props) {
  const { t } = useTranslation();

  return (
    <HoverInfo content={disabled ? disabledTitle : t("composer.addMenu.hoverLabel")}><Button
      type="button"
      disabled={disabled}

      aria-label={t("composer.addMenu.hoverLabel")}
      onClick={onClick}
      className={cn(
        "group/add flex h-8 w-8 shrink-0 items-center gap-0 overflow-hidden rounded-full bg-ui-tint p-0 text-text-secondary shadow-none",
        "hover:bg-ui-tint-strong hover:text-text-base",
        disabled && "cursor-not-allowed opacity-40",
      )}
    >
      <span
        className={cn(
          "flex h-8 w-8 shrink-0 items-center justify-center",
        )}
      >
        <FontAwesomeIcon icon={["fas", "plus"]} className="text-[11px]" />
      </span>
      <span
        className={cn(
          "sr-only",
        )}
      >
        {t("composer.addMenu.hoverLabel")}
      </span>
    </Button></HoverInfo>
  );
}
