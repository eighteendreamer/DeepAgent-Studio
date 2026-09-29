import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { X } from "lucide-react";

import { useTranslation } from "react-i18next";
import {
  Dialog,
  DialogClose,
  DialogCloseIcon,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "../shadcn/dialog";

export function ComputerSettings() {
  const { t } = useTranslation();

  return (
    <Dialog>
      <div className="pb-20">
        <div className="mb-10 max-w-[700px]">
          <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.computer.title")}</h1>
          <div className="text-[13px] text-text-secondary">
            {t("settings.computer.desc")}
          </div>
        </div>

        <div className="max-w-[700px]">
          <h2 className="text-[14px] font-medium text-text-base mb-3">{t("settings.computer.control")}</h2>
          
          <div className="border border-border-theme rounded-xl p-4 flex items-center justify-between bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] mb-8">
            <div className="flex items-center">
              <div className="relative mr-4">
                <div className="w-10 h-10 flex items-center justify-center">
                  <FontAwesomeIcon icon={["fab", "chrome"]} className="text-[32px] text-blue-500" style={{
                    // Approximate Chrome colors with a gradient or just blue for simplicity
                    background: "-webkit-linear-gradient(45deg, #4285F4, #34A853, #FBBC05, #EA4335)",
                    WebkitBackgroundClip: "text",
                    WebkitTextFillColor: "transparent"
                  }} />
                </div>
                {/* Small overlay icon */}
                <div className="absolute bottom-0 right-0 translate-x-1 translate-y-1">
                  <FontAwesomeIcon icon={["fas", "puzzle-piece"]} className="text-[16px] text-gray-500 drop-shadow-sm" />
                </div>
              </div>
              <div>
                <div className="text-[14px] font-medium text-text-base mb-0.5">Google Chrome</div>
                <div className="text-[12px] text-text-secondary flex items-center">
                  <span className="w-1.5 h-1.5 rounded-full bg-red-500 mr-1.5"></span>
                  {t("settings.computer.extensionNotConnected")}
                </div>
              </div>
            </div>
            <DialogTrigger
              className="px-4 py-1.5 bg-black/5 hover:bg-black/5 rounded-full text-[12px] font-medium text-text-base transition-colors"
            >
              {t("settings.computer.install")}
            </DialogTrigger>
          </div>

          <h2 className="text-[14px] font-medium text-text-base mb-3">{t("settings.computer.alwaysAllowed")}</h2>
          
          <div className="border border-border-theme rounded-xl p-4 bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] flex justify-center items-center h-[60px]">
            <span className="text-[13px] text-text-secondary">{t("settings.computer.none")}</span>
          </div>
        </div>
      </div>

      <DialogContent className="max-w-[560px]" aria-label={t("settings.computer.installChrome")}>
        <DialogHeader className="shrink-0 border-b border-border-theme">
          <div className="flex min-w-0 items-center gap-3">
            <FontAwesomeIcon icon={["fab", "chrome"]} className="shrink-0 text-[28px] text-blue-500" aria-hidden="true" />
            <div className="min-w-0">
              <DialogTitle>{t("settings.computer.installChrome")}</DialogTitle>
              <DialogDescription>{t("settings.computer.developedBy")}</DialogDescription>
            </div>
          </div>
          <DialogCloseIcon aria-label={t("settings.computer.close")}>
            <X className="h-4 w-4" aria-hidden="true" />
          </DialogCloseIcon>
        </DialogHeader>

        <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
          <div className="rounded-xl border border-border-theme bg-white p-5">
            <div className="mb-4">
              <div className="mb-1 flex items-center">
                <span className="mr-2 text-[14px] font-medium text-text-base">Chrome</span>
                <span className="rounded border border-border-theme px-2 py-0.5 text-[11px] text-text-secondary">openai-bundled</span>
              </div>
              <div className="mb-0.5 text-[12px] text-text-secondary">{t("settings.computer.providedBy")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.computer.category")}</div>
            </div>

            <div className="mb-4">
              <h3 className="mb-2 text-[13px] font-medium text-text-base">{t("settings.computer.about")}</h3>
              <p className="text-[12px] leading-relaxed text-text-secondary">
                Chrome lets Codex use your Chrome browser for tasks that need your existing browser state, including open tabs, page content, and websites you're already signed into. It can navigate, view, click, type, and take screenshots while working. You stay in control: Codex asks before interacting with new sites, you can stop actions at any time, and you can manage or remove Chrome access in settings. Browser content may include sensitive information from logged-in sites. Browser data from using this plugin may be used for training, subject to your OpenAI account data controls.
              </p>
            </div>

            <div className="mb-4">
              <h3 className="mb-2 text-[13px] font-medium text-text-base">{t("settings.computer.includes")}</h3>
              <div className="mb-2">
                <div className="mb-1 text-[12px] text-text-secondary">{t("settings.computer.browserExtension")}</div>
                <span className="inline-block rounded-md border border-gray-200 bg-gray-50 px-2 py-1 text-[12px] text-text-secondary">Codex Chrome Extension</span>
              </div>
              <div>
                <div className="mb-1 text-[12px] text-text-secondary">{t("settings.computer.skills")}</div>
                <span className="inline-block rounded-md border border-gray-200 bg-gray-50 px-2 py-1 text-[12px] text-text-secondary">Chrome</span>
              </div>
            </div>

            <div>
              <h3 className="mb-2 text-[13px] font-medium text-text-base">{t("settings.computer.features")}</h3>
              <div className="flex gap-2">
                <span className="rounded-md border border-gray-200 bg-gray-50 px-2 py-1 text-[12px] text-text-secondary">Interactive</span>
                <span className="rounded-md border border-gray-200 bg-gray-50 px-2 py-1 text-[12px] text-text-secondary">Read</span>
              </div>
            </div>
          </div>
        </div>

        <DialogFooter className="shrink-0 border-t border-border-theme">
          <DialogClose className="w-full rounded-xl bg-black py-2.5 text-[14px] font-medium text-white transition-colors hover:bg-gray-800">
            {t("settings.computer.installChrome")}
          </DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
