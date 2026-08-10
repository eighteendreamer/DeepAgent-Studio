import gsap from "gsap";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { Label } from "../shadcn/label";
import { cn } from "../shadcn/utils";
import { MOTION } from "./motion";
import { prefersReducedMotion } from "./modalFromOriginMotion";

export interface CopyContactFieldProps {
  id: string;
  label: string;
  value: string;
  copyLabel: string;
  copiedLabel: string;
}

const TIP_LERP = 0.065;
const TIP_OFFSET = { x: 14, y: -12 };
const TIP_Z_INDEX = 250;

/**
 * 联系信息只读行 —— 纯 tint 块；hover 时毛玻璃提示 lerp 慢跟鼠标，可超出字段框。
 */
export function CopyContactField({ id, label, value, copyLabel, copiedLabel }: CopyContactFieldProps) {
  const rowRef = useRef<HTMLButtonElement>(null);
  const tipRef = useRef<HTMLSpanElement>(null);
  const rafRef = useRef(0);
  const visibleRef = useRef(false);
  const hoveringRef = useRef(false);
  const targetRef = useRef({ x: 0, y: 0 });
  const currentRef = useRef({ x: 0, y: 0 });
  const [copied, setCopied] = useState(false);
  const [tipMounted, setTipMounted] = useState(false);
  const pendingClientRef = useRef<{ x: number; y: number } | null>(null);

  const applyTipPosition = useCallback(() => {
    const tip = tipRef.current;
    if (!tip) return;
    tip.style.transform = `translate3d(${currentRef.current.x}px, ${currentRef.current.y}px, 0)`;
  }, []);

  const clampToViewport = useCallback((x: number, y: number) => {
    const tip = tipRef.current;
    const tipW = tip?.offsetWidth ?? 72;
    const tipH = tip?.offsetHeight ?? 24;
    return {
      x: Math.min(Math.max(x, 8), window.innerWidth - tipW - 8),
      y: Math.min(Math.max(y, 8), window.innerHeight - tipH - 8),
    };
  }, []);

  const setTargetFromClient = useCallback(
    (clientX: number, clientY: number) => {
      targetRef.current = clampToViewport(clientX + TIP_OFFSET.x, clientY + TIP_OFFSET.y);
    },
    [clampToViewport],
  );

  const tick = useCallback(() => {
    if (!visibleRef.current) {
      rafRef.current = 0;
      return;
    }
    currentRef.current.x += (targetRef.current.x - currentRef.current.x) * TIP_LERP;
    currentRef.current.y += (targetRef.current.y - currentRef.current.y) * TIP_LERP;
    applyTipPosition();
    rafRef.current = requestAnimationFrame(tick);
  }, [applyTipPosition]);

  const beginFollowLoop = useCallback(() => {
    if (rafRef.current !== 0 || prefersReducedMotion()) return;
    rafRef.current = requestAnimationFrame(tick);
  }, [tick]);

  const cancelFollowLoop = useCallback(() => {
    cancelAnimationFrame(rafRef.current);
    rafRef.current = 0;
  }, []);

  const startFollow = useCallback((clientX: number, clientY: number) => {
    visibleRef.current = true;
    hoveringRef.current = true;
    pendingClientRef.current = { x: clientX, y: clientY };
    setTipMounted(true);
  }, []);

  const stopFollow = useCallback(() => {
    const tip = tipRef.current;
    visibleRef.current = false;
    hoveringRef.current = false;
    cancelFollowLoop();
    if (!tip) {
      setTipMounted(false);
      pendingClientRef.current = null;
      return;
    }
    if (prefersReducedMotion()) {
      gsap.set(tip, { autoAlpha: 0 });
      setTipMounted(false);
      pendingClientRef.current = null;
      return;
    }
    gsap.to(tip, {
      autoAlpha: 0,
      duration: 0.14,
      ease: "power2.in",
      onComplete: () => {
        setTipMounted(false);
        pendingClientRef.current = null;
      },
    });
  }, [cancelFollowLoop]);

  useLayoutEffect(() => {
    if (!tipMounted || !pendingClientRef.current) return;
    const { x, y } = pendingClientRef.current;
    setTargetFromClient(x, y);
    currentRef.current = { ...targetRef.current };
    const tip = tipRef.current;
    if (!tip) return;
    applyTipPosition();
    if (prefersReducedMotion()) {
      gsap.set(tip, { autoAlpha: 1 });
      return;
    }
    gsap.fromTo(tip, { autoAlpha: 0 }, { autoAlpha: 1, duration: 0.16, ease: "power2.out" });
    beginFollowLoop();
  }, [tipMounted, applyTipPosition, beginFollowLoop, setTargetFromClient]);

  useEffect(
    () => () => {
      visibleRef.current = false;
      cancelFollowLoop();
    },
    [cancelFollowLoop],
  );

  const handleCopy = async () => {
    if (!value) return;
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(value);
      }
    } catch {
      return;
    }
    setCopied(true);
    const tip = tipRef.current;
    if (tip) {
      gsap.set(tip, { autoAlpha: 1 });
    }
    window.setTimeout(() => {
      setCopied(false);
      if (!hoveringRef.current) stopFollow();
    }, 1200);
  };

  const tipNode =
    tipMounted &&
    createPortal(
      <span
        ref={tipRef}
        className={cn("contact-copy-tip rounded-[10px]", copied && "is-copied")}
        style={{ zIndex: TIP_Z_INDEX }}
        aria-hidden="true"
      >
        {copied ? copiedLabel : copyLabel}
      </span>,
      document.body,
    );

  return (
    <div className="grid gap-2">
      <Label htmlFor={id}>{label}</Label>
      <button
        ref={rowRef}
        type="button"
        id={id}
        className={cn(
          "contact-copy-field relative w-full rounded-md bg-ui-tint px-3.5 py-2.5 text-left cursor-copy",
          MOTION.fast,
          copied && "is-copied",
        )}
        onMouseEnter={(e) => startFollow(e.clientX, e.clientY)}
        onMouseMove={(e) => {
          setTargetFromClient(e.clientX, e.clientY);
          if (visibleRef.current && rafRef.current === 0 && !prefersReducedMotion()) {
            beginFollowLoop();
          }
        }}
        onMouseLeave={() => {
          hoveringRef.current = false;
          if (!copied) stopFollow();
          else cancelFollowLoop();
        }}
        onClick={() => void handleCopy()}
      >
        <span className="block text-[13px] leading-snug text-text-base break-all">{value}</span>
      </button>
      {tipNode}
    </div>
  );
}
