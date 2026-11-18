import {
  forwardRef,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  type HTMLAttributes,
  type MouseEvent,
  type MutableRefObject,
  type ReactNode,
  type Ref,
} from "react";
import { createPortal } from "react-dom";

import { cn } from "../shadcn/utils";
import {
  playModalOriginOpen,
  type ModalTriggerOrigin,
} from "./modalFromOriginMotion";
import { Panel } from "./Panel";
import { SurfaceVisibility } from "./SurfaceVisibility";

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
  panelProps?: Omit<HTMLAttributes<HTMLDivElement>, "children" | "className">;
  panelRef?: Ref<HTMLDivElement>;
  backdropClassName?: string;
  closeOnBackdrop?: boolean;
  closeOnEscape?: boolean;
  zIndexClass?: string;
}

/**
 * Controlled modal: opening is synchronous; dismissal never waits for an animation.
 */
export function GlobalModal({
  open,
  onClose,
  children,
  origin = null,
  className,
  panelClassName,
  panelProps,
  panelRef,
  backdropClassName,
  closeOnBackdrop = true,
  closeOnEscape = true,
  zIndexClass = "z-[200]",
}: GlobalModalProps) {
  const visible = useContext(SurfaceVisibility);
  const backdropRef = useRef<HTMLDivElement>(null);
  const panelElementRef = useRef<HTMLDivElement | null>(null);
  const tlRef = useRef<ReturnType<typeof playModalOriginOpen> | null>(null);

  useLayoutEffect(() => {
    if (!open || !visible) return;

    const backdrop = backdropRef.current;
    const panel = panelElementRef.current;
    if (!backdrop || !panel) return;

    tlRef.current?.kill();
    tlRef.current = playModalOriginOpen(backdrop, panel, origin);

    return () => {
      tlRef.current?.kill();
    };
  }, [open, origin, visible]);

  useEffect(() => {
    if (open && !visible) onClose();
  }, [open, visible, onClose]);

  useEffect(() => {
    if (!open || !visible || !closeOnEscape) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose, closeOnEscape, open, visible]);

  useEffect(
    () => () => {
      tlRef.current?.kill();
    },
    [],
  );

  if (!open || !visible || typeof document === "undefined") return null;

  const handleShellMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    if (!closeOnBackdrop) return;
    if (event.target === event.currentTarget || event.target === backdropRef.current) {
      onClose();
    }
  };

  const { onMouseDown: onPanelMouseDown, ...restPanelProps } = panelProps ?? {};

  const setPanelRef = (element: HTMLDivElement | null) => {
    panelElementRef.current = element;
    if (typeof panelRef === "function") {
      panelRef(element);
    } else if (panelRef) {
      (panelRef as MutableRefObject<HTMLDivElement | null>).current = element;
    }
  };

  return createPortal(
    <div
      className={cn("fixed inset-0 flex items-center justify-center px-4", zIndexClass, className)}
      onMouseDown={handleShellMouseDown}
    >
      <div
        ref={backdropRef}
        className={cn("absolute inset-0 bg-black/20", backdropClassName)}
        aria-hidden="true"
      />
      <Panel
        ref={setPanelRef}
        menu={false}
        {...restPanelProps}
        className={cn(
          "relative flex max-h-[88vh] w-full max-w-[620px] flex-col overflow-hidden",
          panelClassName,
        )}
        onMouseDown={(event) => {
          event.stopPropagation();
          onPanelMouseDown?.(event);
        }}
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
