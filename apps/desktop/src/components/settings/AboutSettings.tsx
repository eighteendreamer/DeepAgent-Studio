import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { openExternalUrl } from "../../api";

import {
  Dialog,
  DialogCloseIcon,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "../shadcn/dialog";
import { Avatar, AvatarFallback, AvatarImage } from "../ui/Avatar";
import { CopyContactField } from "../ui/CopyContactField";

const PROJECT_URL = "https://github.com/eighteendreamer/DeepAgent-Studio";
const COPYRIGHT_CERTIFICATE_SRC = "/certificates/software-copyright.jpg";

const STACK = [
  { name: "Tauri", version: "v2", icon: ["far", "window-maximize"] as IconProp },
  { name: "React + TypeScript", version: "18.3 + 5.x", icon: ["fab", "react"] as IconProp },
  { name: "Rust", version: "stable", icon: ["fas", "gear"] as IconProp },
  { name: "Vite", version: "v5", icon: ["fas", "bolt"] as IconProp },
  { name: "DeepSeek API", version: "原生模型层", icon: ["fas", "robot"] as IconProp },
  { name: "SQLite + MCP", version: "持久化与工具扩展", icon: ["fas", "server"] as IconProp },
];

type DeveloperId = "eighteen" | "designer";

const DEVELOPER_ASSETS: Record<
  DeveloperId,
  { avatar: string; wechatQr: string; alipayQr: string; hasSupport: boolean }
> = {
  eighteen: {
    avatar: "/avatars/eighteen.png",
    wechatQr: "/support/eighteen-wechat.jpg",
    alipayQr: "/support/eighteen-alipay.jpg",
    hasSupport: true,
  },
  designer: {
    avatar: "/avatars/designer.png",
    wechatQr: "/support/designer-wechat.jpg",
    alipayQr: "/support/designer-alipay.jpg",
    hasSupport: true,
  },
};

function ContactDialog({ developer }: { developer: DeveloperId }) {
  const { t } = useTranslation();
  const prefix = `settings.about.developers.${developer}`;

  return (
    <Dialog>
      <DialogTrigger className="inline-flex items-center gap-2 rounded-md bg-black/5 px-3 py-2 text-xs font-medium text-text-base transition-colors hover:bg-black/10">
        <FontAwesomeIcon icon={["fas", "circle-user"]} />
        {t("settings.about.actions.contact")}
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <div>
            <DialogTitle>{t("settings.about.contactDialog.title")}</DialogTitle>
            <DialogDescription>{t(`${prefix}.name`)}</DialogDescription>
          </div>
          <DialogCloseIcon aria-label={t("settings.about.actions.close")}>
            <FontAwesomeIcon icon={["fas", "xmark"]} className="text-[14px]" />
          </DialogCloseIcon>
        </DialogHeader>
        <div className="grid gap-5 px-6 py-6">
          {(["email", "phone", "qq", "wechat"] as const).map((field) => {
            const inputId = `${developer}-${field}`;
            return (
              <CopyContactField
                key={field}
                id={inputId}
                label={t(`settings.about.contactDialog.${field}`)}
                value={t(`${prefix}.contact.${field}`)}
                copyLabel={t("settings.about.contactDialog.clickToCopy")}
                copiedLabel={t("settings.about.contactDialog.copied")}
              />
            );
          })}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function SupportDialog({ developer }: { developer: DeveloperId }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const prefix = `settings.about.developers.${developer}`;

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger className="inline-flex items-center gap-2 rounded-md bg-black/5 px-3 py-2 text-xs font-medium text-text-base transition-colors hover:bg-black/10">
        <FontAwesomeIcon icon={["fas", "star"]} />
        {t("settings.about.actions.support")}
      </DialogTrigger>
      <DialogContent className="max-w-[520px]">
        <DialogHeader>
          <div>
            <DialogTitle>{t("settings.about.supportDialog.title")}</DialogTitle>
            <DialogDescription>{t(`${prefix}.name`)}</DialogDescription>
          </div>
          <DialogCloseIcon aria-label={t("settings.about.actions.close")}>
            <FontAwesomeIcon icon={["fas", "xmark"]} className="text-[14px]" />
          </DialogCloseIcon>
        </DialogHeader>
        <div className="flex flex-col items-center gap-4 px-6 py-7 text-center">
          <p className="text-sm font-medium">{t("settings.about.supportDialog.heading")}</p>
          {DEVELOPER_ASSETS[developer].hasSupport ? (
            <div className="grid w-full grid-cols-2 gap-4">
              <figure className="min-w-0">
                <div className="overflow-hidden rounded-md bg-hover-bg">
                  <img
                    src={DEVELOPER_ASSETS[developer].wechatQr}
                    alt={t("settings.about.supportDialog.wechat")}
                    className="block aspect-square h-auto w-full object-cover object-center"
                  />
                </div>
                <figcaption className="mt-2 text-xs text-text-secondary">{t("settings.about.supportDialog.wechat")}</figcaption>
              </figure>
              <figure className="min-w-0">
                <div className="overflow-hidden rounded-md bg-hover-bg">
                  <img
                    src={DEVELOPER_ASSETS[developer].alipayQr}
                    alt={t("settings.about.supportDialog.alipay")}
                    className="block aspect-square h-auto w-full object-cover object-center"
                  />
                </div>
                <figcaption className="mt-2 text-xs text-text-secondary">{t("settings.about.supportDialog.alipay")}</figcaption>
              </figure>
            </div>
          ) : (
            <div className="flex h-44 w-full items-center justify-center rounded-md bg-hover-bg text-sm text-text-secondary">
              {t("settings.about.supportDialog.notAvailable")}
            </div>
          )}
          <p className="max-w-sm text-xs leading-5 text-text-secondary">{t("settings.about.supportDialog.pending")}</p>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function DeveloperAvatar({ developer }: { developer: DeveloperId }) {
  const assets = DEVELOPER_ASSETS[developer];
  const { t } = useTranslation();
  const name = t(`settings.about.developers.${developer}.name`);

  return (
    <Avatar className="h-12 w-12">
      <AvatarImage src={assets.avatar} alt={name} />
      <AvatarFallback>
        <FontAwesomeIcon icon={["fas", "circle-user"]} aria-hidden="true" />
      </AvatarFallback>
    </Avatar>
  );
}

function DeveloperBlock({ developer }: { developer: DeveloperId }) {
  const { t } = useTranslation();
  const prefix = `settings.about.developers.${developer}`;

  return (
    <section className="min-w-0 border-t border-border-theme pt-5">
      <div className="mb-4 flex items-start justify-between gap-4">
        <div>
          <h3 className="text-base font-semibold">{t(`${prefix}.name`)}</h3>
          <p className="mt-1 text-xs text-text-secondary">{t(`${prefix}.role`)}</p>
        </div>
        <DeveloperAvatar developer={developer} />
      </div>
      <p className="min-h-20 text-sm leading-6 text-text-secondary">{t(`${prefix}.bio`)}</p>
      <a
        href={t(`${prefix}.repository`)}
        onClick={(event) => {
          if ((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) {
            event.preventDefault();
            void openExternalUrl(t(`${prefix}.repository`));
          }
        }}
        className="mt-4 inline-flex max-w-full items-center gap-2 truncate text-xs text-text-secondary hover:text-text-base"
      >
        <FontAwesomeIcon icon={["fab", "github"]} className="shrink-0" />
        <span className="truncate">{t("settings.about.contact.repository")}</span>
        <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="shrink-0" />
      </a>
      <div className="mt-5 flex flex-wrap gap-2">
        <ContactDialog developer={developer} />
        <SupportDialog developer={developer} />
      </div>
    </section>
  );
}

// 软著证书模块：以摘要信息承载可信背书，点击缩略图查看原件。
function CopyrightCertificateSection() {
  const { t } = useTranslation();
  const certificateItems = [
    ["software", t("settings.about.copyright.software")],
    ["registration", t("settings.about.copyright.registration")],
    ["version", t("settings.about.copyright.version")],
    ["date", t("settings.about.copyright.date")],
  ] as const;

  return (
    <section className="border-b border-border-theme py-10">
      <h2 className="text-lg font-semibold">{t("settings.about.copyright.title")}</h2>
      <div className="mt-5 grid gap-8 md:grid-cols-[minmax(0,1fr)_260px] md:items-start">
        <div className="min-w-0">
          <h3 className="text-base font-semibold text-text-base">
            {t("settings.about.copyright.heading")}
          </h3>
          <p className="mt-2 max-w-2xl text-sm leading-6 text-text-secondary">
            {t("settings.about.copyright.description")}
          </p>
          <dl className="mt-5 grid max-w-2xl gap-3 text-sm sm:grid-cols-2">
            {certificateItems.map(([key, value]) => (
              <div key={key} className="min-w-0 border-t border-border-theme/70 pt-3">
                <dt className="text-xs text-text-secondary">
                  {t(`settings.about.copyright.fields.${key}`)}
                </dt>
                <dd className="mt-1 truncate font-medium text-text-base">{value}</dd>
              </div>
            ))}
          </dl>
        </div>

        <Dialog>
          <DialogTrigger className="group w-full appearance-none text-left">
            <figure className="overflow-hidden rounded-lg border border-border-theme bg-white shadow-sm transition-all duration-150 group-hover:-translate-y-0.5 group-hover:border-text-secondary/40 group-hover:shadow-md">
              <img
                src={COPYRIGHT_CERTIFICATE_SRC}
                alt={t("settings.about.copyright.imageAlt")}
                className="block aspect-[3/4] w-full object-cover object-top"
              />
              <figcaption className="flex items-center justify-between gap-3 border-t border-border-theme bg-white px-3 py-2 text-xs text-text-secondary">
                <span>{t("settings.about.copyright.previewCaption")}</span>
                <FontAwesomeIcon icon={["fas", "magnifying-glass-plus"]} className="shrink-0" />
              </figcaption>
            </figure>
          </DialogTrigger>
          <DialogContent className="w-fit max-w-[calc(100vw-2rem)]">
            <DialogHeader>
              <div>
                <DialogTitle>{t("settings.about.copyright.dialogTitle")}</DialogTitle>
                <DialogDescription>{t("settings.about.copyright.registration")}</DialogDescription>
              </div>
              <DialogCloseIcon aria-label={t("settings.about.actions.close")}>
                <FontAwesomeIcon icon={["fas", "xmark"]} className="text-[14px]" />
              </DialogCloseIcon>
            </DialogHeader>
            <div className="min-h-0 overflow-auto bg-sidebar-bg px-4 py-4">
              <img
                src={COPYRIGHT_CERTIFICATE_SRC}
                alt={t("settings.about.copyright.imageAlt")}
                className="block max-h-[72vh] w-auto max-w-[calc(100vw-4rem)] rounded-md border border-border-theme bg-white shadow-sm sm:max-w-[520px]"
              />
            </div>
          </DialogContent>
        </Dialog>
      </div>
    </section>
  );
}

export function AboutSettings() {
  const { t } = useTranslation();

  return (
    <div className="pb-20">
      <section className="border-b border-border-theme pb-10">
        <div className="flex items-baseline justify-between gap-4">
          <h1 className="text-2xl font-semibold text-text-base">DeepAgent Studio</h1>
          <span className="text-xs text-text-secondary">{t("settings.about.project.version")}</span>
        </div>
        <h2 className="mt-8 text-lg font-semibold">{t("settings.about.project.title")}</h2>
        <p className="mt-3 max-w-2xl text-sm leading-6 text-text-secondary">
          {t("settings.about.project.description")}
        </p>
        <div className="mt-8">
          <h2 className="mb-4 text-lg font-semibold">{t("settings.about.stack.title")}</h2>
          <div className="grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">
            {STACK.map((item) => (
              <div key={item.name} className="flex min-w-0 items-center gap-3 py-1">
                <FontAwesomeIcon icon={item.icon} className="w-4 shrink-0 text-text-secondary" />
                <span className="truncate text-sm font-medium">{item.name}</span>
                <span className="ml-auto shrink-0 text-xs text-text-secondary">{item.version}</span>
              </div>
            ))}
          </div>
        </div>
      </section>

      <CopyrightCertificateSection />

      <section className="pt-10">
        <h2 className="mb-6 text-lg font-semibold">{t("settings.about.developer.title")}</h2>
        <div className="grid gap-10 sm:grid-cols-2 sm:gap-8">
          <DeveloperBlock developer="eighteen" />
          <DeveloperBlock developer="designer" />
        </div>
        <a
          href={PROJECT_URL}
          onClick={(event) => {
            if ((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) {
              event.preventDefault();
              void openExternalUrl(PROJECT_URL);
            }
          }}
          className="mt-10 inline-flex items-center gap-2 text-xs text-text-secondary hover:text-text-base"
        >
          <FontAwesomeIcon icon={["fab", "github"]} />
          {t("settings.about.contact.github")}
          <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} />
        </a>
      </section>
    </div>
  );
}
