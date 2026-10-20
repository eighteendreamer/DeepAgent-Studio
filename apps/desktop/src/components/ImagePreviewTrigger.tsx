import { X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "./shadcn/utils";
import {
  Dialog,
  DialogCloseIcon,
  DialogContent,
  DialogTrigger,
} from "./shadcn/dialog";

interface ImagePreviewTriggerProps {
  src: string;
  alt: string;
  triggerClassName?: string;
  imageClassName?: string;
}

export function ImagePreviewTrigger({
  src,
  alt,
  triggerClassName,
  imageClassName,
}: ImagePreviewTriggerProps) {
  const { t } = useTranslation();
  const label = alt.trim()
    ? t("composer.attachments.previewNamedImage", { name: alt })
    : t("composer.attachments.previewImage");

  return (
    <Dialog>
      <DialogTrigger
        aria-label={label}
        aria-haspopup="dialog"
        onClick={(event) => event.stopPropagation()}
        className={cn(
          "group/image-preview relative inline-flex shrink-0 cursor-zoom-in overflow-hidden border-0 bg-transparent p-0 outline-none focus-visible:ring-2 focus-visible:ring-primary focus-visible:ring-offset-2",
          triggerClassName,
        )}
      >
        <img src={src} alt={alt} className={imageClassName} />
        <span
          aria-hidden="true"
          className="pointer-events-none absolute inset-0 bg-black/0 transition-colors group-hover/image-preview:bg-black/10"
        />
      </DialogTrigger>
      <DialogContent
        aria-label={label}
        backdropClassName="!bg-black/85"
        className="!h-fit !max-h-none !w-fit !max-w-none !overflow-visible !rounded-none !bg-transparent !shadow-none"
      >
        <div className="relative flex items-center justify-center">
          <img
            src={src}
            alt={alt}
            className="block max-h-[min(75dvh,760px)] max-w-[min(80vw,1120px)] rounded-lg object-contain shadow-2xl"
          />
          <DialogCloseIcon
            aria-label={t("composer.attachments.closePreview")}
            className="absolute right-3 top-3 z-10 h-9 w-9 rounded-full bg-black/55 text-white hover:bg-black/75 hover:text-white"
          >
            <X aria-hidden="true" className="h-5 w-5" />
          </DialogCloseIcon>
        </div>
      </DialogContent>
    </Dialog>
  );
}
