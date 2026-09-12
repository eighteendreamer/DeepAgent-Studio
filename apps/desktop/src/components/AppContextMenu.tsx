import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "./shadcn/context-menu";

/**
 * 全局右键菜单：拦截浏览器默认菜单，统一换成 shadcn ContextMenu。
 * - 已被业务自行处理的区域（defaultPrevented，如无限画布）不接管；
 * - 带 data-custom-contextmenu 标记的区域不接管（业务行级菜单自管，如远程目录树）；
 * - 合成事件（isTrusted=false）不接管，避免自触发死循环；
 * - F12 开发者工具不受影响。
 */
export function AppContextMenu() {
  const { t } = useTranslation();
  const triggerRef = useRef<HTMLSpanElement>(null);
  const [hasSelection, setHasSelection] = useState(false);

  useEffect(() => {
    const onContextMenu = (event: MouseEvent) => {
      if (!event.isTrusted || event.defaultPrevented) return;
      if (
        event.target instanceof Element &&
        event.target.closest("[data-custom-contextmenu]")
      )
        return;
      event.preventDefault();
      setHasSelection(Boolean(window.getSelection()?.toString()));
      const trigger = triggerRef.current;
      if (!trigger) return;
      trigger.style.left = `${event.clientX}px`;
      trigger.style.top = `${event.clientY}px`;
      trigger.dispatchEvent(
        new MouseEvent("contextmenu", {
          bubbles: true,
          cancelable: true,
          clientX: event.clientX,
          clientY: event.clientY,
        }),
      );
    };
    window.addEventListener("contextmenu", onContextMenu, true);
    return () => window.removeEventListener("contextmenu", onContextMenu, true);
  }, []);

  return (
    <ContextMenu>
      <ContextMenuTrigger
        ref={triggerRef}
        aria-hidden
        className="pointer-events-none fixed left-0 top-0 h-px w-px opacity-0"
      />
      <ContextMenuContent>
        <ContextMenuItem
          disabled={!hasSelection}
          onSelect={() => {
            document.execCommand("copy");
          }}
        >
          {t("contextMenu.copy")}
          <ContextMenuShortcut>Ctrl+C</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuItem
          onSelect={() => {
            document.execCommand("selectAll");
          }}
        >
          {t("contextMenu.selectAll")}
          <ContextMenuShortcut>Ctrl+A</ContextMenuShortcut>
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
