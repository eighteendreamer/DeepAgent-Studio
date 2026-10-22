import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { CircleAlert, Image, LoaderCircle, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import gsap from "gsap";
import type { ComposerAttachment } from "../types";
import { ImagePreviewTrigger } from "./ImagePreviewTrigger";
import { Button } from "./shadcn/button";
import { cn } from "./shadcn/utils";
import { HoverInfo } from "./ui/HoverInfo";
import { IconButton } from "./ui/IconButton";
import { SpringDisclosure } from "./ui/SpringDisclosure";
import { prefersReducedMotion, springEase } from "./ui/springMotion";

function AttachmentMotionItem({ exiting, onExited, children }: {
  exiting: boolean; onExited: () => void; children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const latestExit = useRef(onExited);
  latestExit.current = onExited;
  useLayoutEffect(() => {
    const node = ref.current;
    if (!node) return;
    const reduced = prefersReducedMotion();
    node.inert = exiting;
    const tween = exiting
      ? gsap.to(node, { opacity: 0, scale: 0.95, duration: reduced ? 0 : 0.16,
          onComplete: () => latestExit.current() })
      : gsap.fromTo(node, { opacity: 0, y: 6, scale: 0.96 }, { opacity: 1, y: 0, scale: 1,
          duration: reduced ? 0 : 0.42, ease: springEase(360, 24, 0.42) });
    return () => { tween.kill(); };
  }, [exiting]);
  return <div ref={ref} className="shrink-0">{children}</div>;
}

interface ComposerAttachmentListProps {
  attachments: ComposerAttachment[];
  onRemove: (id: string) => void;
}

export function ComposerAttachmentList({ attachments, onRemove }: ComposerAttachmentListProps) {
  const { t } = useTranslation();
  const [retained, setRetained] = useState(attachments);
  const activeIds = new Set(attachments.map((item) => item.id));
  const shown = [...attachments, ...retained.filter((item) => !activeIds.has(item.id))];
  useEffect(() => {
    setRetained((previous) => [...attachments, ...previous.filter((item) => !attachments.some((active) => active.id === item.id))]);
  }, [attachments]);
  const removeExited = (id: string) => setRetained((items) => items.filter((item) => item.id !== id));

  return (
    <SpringDisclosure open={shown.length > 0} unmountOnExit animateOnMount>
    <div className="-mx-1 mb-2 flex max-w-full items-start gap-2.5 overflow-x-auto px-1 pb-1 pt-1.5">
      {shown.map((item) => {
        if (item.kind === "image") {
          const failed = item.status === "error";
          const status = failed
            ? t("composer.attachments.failed")
            : item.status === "processing"
            ? t("composer.attachments.recognizing")
            : null;
          const description = status
            ? t("composer.attachments.statusLabel", {
                name: item.name,
                status: failed ? item.error ?? status : status,
              })
            : item.name;

          return (
            <AttachmentMotionItem key={item.id} exiting={!activeIds.has(item.id)} onExited={() => removeExited(item.id)}><HoverInfo
              key={item.id}
              content={description}
            >
              <div className="group/attachment relative flex h-[68px] w-[68px] shrink-0">
                {item.dataUrl ? (
                  <ImagePreviewTrigger
                    src={item.dataUrl}
                    alt={item.name}
                    triggerClassName="h-full w-full rounded-[10px] border border-border-theme bg-ui-tint"
                    imageClassName="h-full w-full object-cover"
                  />
                ) : (
                  <div className="flex h-full w-full items-center justify-center rounded-[10px] border border-border-theme bg-ui-tint text-text-secondary">
                    <Image aria-hidden="true" className="h-5 w-5" />
                  </div>
                )}
                {status && (
                  <span
                    role={failed ? "alert" : "status"}
                    aria-label={description}
                    className={cn(
                      "pointer-events-none absolute inset-x-1 bottom-1 flex items-center justify-center gap-0.5 whitespace-nowrap rounded-md bg-elevated-bg px-0.5 py-0.5 text-[10px] leading-4",
                      failed ? "text-red-500" : "text-text-secondary",
                    )}
                  >
                    {failed ? (
                      <CircleAlert aria-hidden="true" className="h-3 w-3 shrink-0" />
                    ) : (
                      <LoaderCircle aria-hidden="true" className="h-3 w-3 shrink-0 motion-safe:animate-spin" />
                    )}
                    <span className="min-w-0 truncate">{status}</span>
                  </span>
                )}
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={t("composer.attachments.removeImage", { name: item.name })}
                  onClick={() => onRemove(item.id)}
                  className="absolute -right-1 -top-1 z-10 h-5 w-5 rounded-full border-2 border-elevated-bg bg-neutral-600 p-0 text-white opacity-0 transition-opacity hover:bg-neutral-800 hover:text-white group-hover/attachment:opacity-100 group-focus-within/attachment:opacity-100 motion-reduce:transition-none [@media(hover:none)]:opacity-100"
                >
                  <X aria-hidden="true" className="h-2.5 w-2.5" />
                </Button>
              </div>
            </HoverInfo></AttachmentMotionItem>
          );
        }

        return (
          <AttachmentMotionItem key={item.id} exiting={!activeIds.has(item.id)} onExited={() => removeExited(item.id)}><div
            key={item.id}
            className="flex h-14 max-w-[220px] shrink-0 items-center gap-2 rounded-2xl bg-black/5 px-2 text-left"
          >
            <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-white text-text-secondary">
              <FontAwesomeIcon icon={["fas", item.kind === "text" ? "file-lines" : "file"]} />
            </div>
            <div className="min-w-0 flex-1">
              <div className="truncate text-[12px] font-medium text-text-base">{item.name}</div>
              <div className="truncate text-[11px] text-text-secondary">
                {item.status === "processing"
                  ? t("composer.attachments.processing")
                  : item.status === "error"
                  ? item.error ?? t("composer.attachments.failed")
                  : item.kind === "text"
                  ? t("composer.attachments.text")
                  : t("composer.attachments.file")}
              </div>
            </div>
            <IconButton
              type="button"
              aria-label={t("composer.attachments.removeAttachment", { name: item.name })}
              onClick={() => onRemove(item.id)}
              className="h-6 w-6 rounded-full"
            >
              <FontAwesomeIcon icon={["fas", "xmark"]} className="text-[11px]" />
            </IconButton>
          </div></AttachmentMotionItem>
        );
      })}
    </div>
    </SpringDisclosure>
  );
}
