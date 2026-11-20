import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { ContextMenuContent, ContextMenuItem, ContextMenuSeparator } from "../components/shadcn/base-context-menu";
import { openMain, resetPet, updatePet, type Character } from "./api";

const actionGroups = [
  { id: "click", label: "desktopPet.clickActions" },
  { id: "move", label: "desktopPet.moveActions" },
  { id: "小动作", label: "desktopPet.smallActions" },
  { id: "玩耍", label: "desktopPet.playActions" },
  { id: "吃什么", label: "desktopPet.foodActions" },
  { id: "时节", label: "desktopPet.seasonActions" },
] as const;

type View = "main" | "groups" | { group: string };

interface Props {
  character: Character;
  open: boolean;
  onPlay: (name: string) => void;
  onError: (error: unknown) => void;
}

export function PetMenu({ character, open, onPlay, onError }: Props) {
  const { t } = useTranslation();
  const [view, setView] = useState<View>("main");
  const first = useRef<HTMLElement>(null);
  const navigating = useRef(false);
  const groups = actionGroups.flatMap(group => {
    const names = character.groups.find(item => item.id === group.id)?.actions.filter(name => character.actions[name]);
    return names?.length ? [{ ...group, names }] : [];
  });
  const group = typeof view === "object" ? groups.find(item => item.id === view.group) : undefined;

  useEffect(() => { if (!open) setView("main"); }, [open]);
  useLayoutEffect(() => {
    if (!navigating.current) return;
    navigating.current = false;
    const row = first.current;
    if (row) {
      if (row.parentElement) row.parentElement.scrollTop = 0;
      row.focus({ preventScroll: true });
    }
  }, [view]);
  const go = (next: View) => () => {
    navigating.current = true;
    setView(next);
  };
  const left = <ChevronLeft size={13} className="text-neutral-500" aria-hidden="true" />;
  const right = <ChevronRight size={13} className="ml-auto text-neutral-500" aria-hidden="true" />;

  return <ContextMenuContent className="pet-context-menu bg-white text-neutral-900 shadow-none [--ui-menu-border:#e4e4e7]">
    {view === "main" ? <>
      {groups.length > 0 && <>
        <ContextMenuItem ref={first} closeOnClick={false} onClick={go("groups")}>
          <span>{t("desktopPet.actions")}</span>{right}
        </ContextMenuItem>
        <ContextMenuSeparator />
      </>}
      <ContextMenuItem onClick={() => void openMain().catch(onError)}>{t("desktopPet.openMain")}</ContextMenuItem>
      <ContextMenuItem onClick={() => void updatePet({ sleeping: true }).catch(onError)}>{t("desktopPet.sleep")}</ContextMenuItem>
      <ContextMenuItem onClick={() => void resetPet().catch(onError)}>{t("desktopPet.reset")}</ContextMenuItem>
      <ContextMenuItem disabled>{t("desktopPet.chat")}</ContextMenuItem>
    </> : <>
      <ContextMenuItem ref={first} closeOnClick={false} onClick={go(group ? "groups" : "main")}>{left}{t("desktopPet.menuBack")}</ContextMenuItem>
      <ContextMenuSeparator />
      {group ? group.names.map(name => (
        <ContextMenuItem key={name} onClick={() => onPlay(name)}>{name}</ContextMenuItem>
      )) : groups.map(item => (
        <ContextMenuItem key={item.id} closeOnClick={false} onClick={go({ group: item.id })}><span>{t(item.label)}</span>{right}</ContextMenuItem>
      ))}
    </>}
  </ContextMenuContent>;
}
