import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleUser, LogOut, PawPrint, Settings } from "lucide-react";
import { MENU_ITEM_ATTR, SlidingMenuList } from "./ui/SlidingMenuList";
import { MENU_LIST, MOTION } from "./ui/motion";
import { MorphingMenuShell } from "./ui/MorphingMenuShell";
import { HoverInfo } from "./ui/HoverInfo";
import { cn } from "./shadcn/utils";
import { Button } from "./shadcn/button";
import { message } from "./message";
import { getPetPreferences, isPetAwake, listenPet, nativePet, updatePet, type Preferences } from "../pet/api";

const SETTINGS_MENU = {
  pad: "px-2 py-1.5",
  row: "relative z-[1] flex w-full cursor-pointer items-center rounded-lg px-2.5 py-2 text-left text-[13px] text-text-base whitespace-nowrap hover:bg-transparent",
  icon: "mr-2.5 h-4 w-4 shrink-0 text-text-secondary",
  pill: "left-0 right-0 rounded-lg",
  account: "flex min-w-0 items-center px-2.5 py-2 text-[13px] font-medium text-text-base",
} as const;

type Props = {
  onOpenSettings: () => void;
  onLogout: () => void;
  compact?: boolean;
};

/** Share the model menu's persistent button/panel shell, including close and
 * rapid reopen. Keep the real button rather than cloning its outer container. */
export function SidebarSettingsMenu({ onOpenSettings, onLogout, compact = false }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [petPreferences, setPetPreferences] = useState<Preferences>();
  const [petBusy, setPetBusy] = useState(false);
  useEffect(() => {
    if (!nativePet || !open) return;
    let live = true;
    let receivedUpdate = false;
    const listener = listenPet<Preferences>("preferences", preferences => {
      receivedUpdate = true;
      if (live) setPetPreferences(preferences);
    });
    void listener.then(getPetPreferences).then(preferences => {
      if (live && !receivedUpdate) setPetPreferences(preferences);
    }).catch(error => { if (live) message.error(String(error)); });
    return () => {
      live = false;
      setPetPreferences(undefined);
      void listener.then(unlisten => unlisten(), () => undefined);
    };
  }, [open]);
  const petAwake = isPetAwake(petPreferences);
  const togglePet = async () => {
    if (!petPreferences || petBusy) return;
    setPetBusy(true);
    try {
      await updatePet({ enabled: !petAwake });
      setOpen(false);
    } catch (error) {
      message.error(String(error));
    } finally {
      setPetBusy(false);
    }
  };
  const layoutId = useId().replace(/:/g, "");
  const run = (action: () => void) => { setOpen(false); action(); };
  const menuContent = (
    <div className={SETTINGS_MENU.pad} onClick={(e) => e.stopPropagation()}>
      <div className={SETTINGS_MENU.account}>
        <CircleUser className="mr-2.5 h-4 w-4 shrink-0 text-text-secondary" />
        <span className="truncate">{t("sidebar.loginApi")}</span>
      </div>
      <div className={MENU_LIST.divider} />
      <SlidingMenuList activeId="__none__" pillClassName={SETTINGS_MENU.pill}>
        <div
          {...{ [MENU_ITEM_ATTR]: "settings" }}
          className={cn(SETTINGS_MENU.row, MOTION.fast)}
          onClick={() => run(onOpenSettings)}
        >
          <Settings className={SETTINGS_MENU.icon} />
          {t("sidebar.settings")}
        </div>
        {nativePet && <Button
          {...{ [MENU_ITEM_ATTR]: "desktop-pet" }}
          variant="ghost"
          className={cn(SETTINGS_MENU.row, "h-auto justify-start gap-0 font-normal", MOTION.fast)}
          disabled={!petPreferences || petBusy}
          onClick={() => void togglePet()}
        >
          <PawPrint className={SETTINGS_MENU.icon} />
          {t(!petPreferences ? "desktopPet.enabled" : petAwake ? "desktopPet.turnOff" : "desktopPet.turnOn")}
        </Button>}
        <div
          {...{ [MENU_ITEM_ATTR]: "logout" }}
          className={cn(SETTINGS_MENU.row, MOTION.fast)}
          onClick={() => run(onLogout)}
        >
          <LogOut className={SETTINGS_MENU.icon} />
          {t("sidebar.logout")}
        </div>
      </SlidingMenuList>
    </div>
  );

  return <MorphingMenuShell
    open={open} onOpenChange={setOpen} layoutId={`sidebar-settings-${layoutId}`}
    className={compact ? undefined : "w-full"} triggerLayout={compact ? "inline" : "block"}
    panelClassName="w-[224px]" panelAlign="left" panelPlacement="above" zIndex={200}
    staggerContent
    trigger={<HoverInfo content={compact ? t("sidebar.settings") : undefined} side="right">
      <Button variant="ghost" aria-label={t("sidebar.settings")} aria-expanded={open}
        className={cn(
          compact ? "app-navigation-button" : "flex h-auto w-full justify-start rounded-md px-2.5 py-1.5 text-sm text-text-base",
          MOTION.fast, open ? "bg-ui-tint" : "hover:bg-ui-tint",
        )} onClick={() => setOpen(value => !value)}>
        <Settings className="h-[18px] w-[18px] shrink-0 text-text-secondary" />
        {!compact && <span className="ml-0.5">{t("sidebar.settings")}</span>}
      </Button>
    </HoverInfo>}
  >{menuContent}</MorphingMenuShell>;
}
