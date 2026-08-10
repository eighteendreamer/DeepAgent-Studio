import {
  forwardRef,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type HTMLAttributes,
  type MouseEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { cn } from "../shadcn/utils";
import {
  playModalOriginClose,
  playModalOriginOpen,
  type ModalTriggerOrigin,
} from "./modalFromOriginMotion";
import { Panel } from "./Panel";

export type { ModalTriggerOrigin } from "./modalFromOriginMotion";
export { modalOriginFromElement } from "./modalFromOriginMotion";

export interface GlobalModalProps {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  /** 触发按钮中心；方案 A 从该点 scale 展开/收回 */
  origin?: ModalTriggerOrigin | null;
  className?: string;
  panelClassName?: string;
  closeOnBackdrop?: boolean;
  closeOnEscape?: boolean;
  zIndexClass?: string;
}

/**
 * 全局居中弹窗 —— portal + GSAP 从触发原点 scale 展开/收回（方案 A）。
 */
export function GlobalModal({
  open,
  onClose,
  children,
  origin = null,
  className,
  panelClassName,
  closeOnBackdrop = true,
  closeOnEscape = true,
  zIndexClass = "z-[200]",
}: GlobalModalProps) {
  const [mounted, setMounted] = useState(open);
  const backdropRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const originRef = useRef<ModalTriggerOrigin | null>(origin);
  const tlRef = useRef<ReturnType<typeof playModalOriginOpen> | null>(null);
  const closingRef = useRef(false);

  useEffect(() => {
    if (open && origin) {
      originRef.current = origin;
    }
  }, [open, origin]);

  const finishClose = useCallback(() => {
    closingRef.current = false;
    setMounted(false);
    originRef.current = null;
    onClose();
  }, [onClose]);

  const beginClose = useCallback(() => {
    if (closingRef.current || !mounted) return;

    const backdrop = backdropRef.current;
    const panel = panelRef.current;
    if (!backdrop || !panel) {
      finishClose();
      return;
    }

    closingRef.current = true;
    tlRef.current?.kill();
    tlRef.current = playModalOriginClose(backdrop, panel, originRef.current);
    tlRef.current.eventCallback("onComplete", finishClose);
  }, [finishClose, mounted]);

  useEffect(() => {
    if (open) {
      setMounted(true);
      closingRef.current = false;
      return;
    }
    if (mounted && !closingRef.current) {
      beginClose();
    }
  }, [beginClose, mounted, open]);

  useLayoutEffect(() => {
    if (!mounted || !open || closingRef.current) return;

    const backdrop = backdropRef.current;
    const panel = panelRef.current;
    if (!backdrop || !panel) return;

    tlRef.current?.kill();
    tlRef.current = playModalOriginOpen(backdrop, panel, originRef.current);

    return () => {
      tlRef.current?.kill();
    };
  }, [mounted, open]);

  useEffect(() => {
    if (!mounted || !closeOnEscape) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") beginClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [beginClose, closeOnEscape, mounted]);

  useEffect(
    () => () => {
      tlRef.current?.kill();
    },
    [],
  );

  if (!mounted || typeof document === "undefined") return null;

  const handleShellMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    if (!closeOnBackdrop || closingRef.current) return;
    if (event.target === event.currentTarget || event.target === backdropRef.current) {
      beginClose();
    }
  };

  return createPortal(
    <div
      className={cn("fixed inset-0 flex items-center justify-center px-4", zIndexClass, className)}
      onMouseDown={handleShellMouseDown}
    >
      <div ref={backdropRef} className="absolute inset-0 bg-black/20" aria-hidden="true" />
      <Panel
        ref={panelRef}
        menu={false}
        className={cn(
          "relative flex max-h-[88vh] w-full max-w-[620px] flex-col overflow-hidden",
          panelClassName,
        )}
        onMouseDown={(event) => event.stopPropagation()}
      >
        {children}
      </Panel>
    </div>,
    document.body,
  );
}

export function GlobalModalHeader({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("flex items-start justify-between gap-4 px-6 py-5", className)} {...props} />;
}

export function GlobalModalFooter({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("flex justify-end gap-2 px-6 py-4", className)} {...props} />;
}

export const GlobalModalTitle = forwardRef<HTMLHeadingElement, HTMLAttributes<HTMLHeadingElement>>(
  function GlobalModalTitle({ className, ...props }, ref) {
    return <h2 ref={ref} className={cn("text-xl font-semibold text-text-base", className)} {...props} />;
  },
);

export const GlobalModalDescription = forwardRef<
  HTMLParagraphElement,
  HTMLAttributes<HTMLParagraphElement>
>(function GlobalModalDescription({ className, ...props }, ref) {
  return <p ref={ref} className={cn("mt-1 text-[13px] leading-5 text-text-secondary", className)} {...props} />;
});

export function GlobalModalBody({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("px-6 py-6", className)} {...props} />;
}
