import type { ReactNode } from "react";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { ToolbarMenuTrigger } from "./ToolbarMenuTrigger";
import { MorphingMenuShell } from "./MorphingMenuShell";

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  layoutId: string;
  icon: IconProp;
  label: ReactNode;
  trailing?: ReactNode;
  title?: string;
  panelClassName?: string;
  zIndex?: number;
  staggerContent?: boolean;
  unstyled?: boolean;
  children: ReactNode;
};

/** Footer 工具栏入口 —— MorphingMenuShell + ToolbarMenuTrigger */
export function MorphingToolbarMenu({
  open,
  onOpenChange,
  layoutId,
  icon,
  label,
  trailing,
  title,
  panelClassName,
  zIndex = 60,
  children,
  staggerContent = true,
  unstyled = false,
}: Props) {
  const trigger = (
    <ToolbarMenuTrigger
      open={open}
      icon={icon}
      label={label}
      trailing={trailing}
      title={title}
      onClick={() => onOpenChange(!open)}
    />
  );

  return (
    <MorphingMenuShell
      open={open}
      onOpenChange={onOpenChange}
      layoutId={layoutId}
      trigger={trigger}
      className="shrink-0"
      panelClassName={panelClassName}
      panelAlign="left"
      zIndex={zIndex}
      staggerContent={staggerContent}
      unstyled={unstyled}
    >
      {children}
    </MorphingMenuShell>
  );
}
