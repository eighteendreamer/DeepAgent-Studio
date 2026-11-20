import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { ContextMenuContent, ContextMenuItem, ContextMenuSeparator } from "../components/shadcn/context-menu";
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
  const first = useRef<HTMLDivElement>(null);
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
  const go = (next: View) => (event: Event) => {
    event.preventDefault();
    navigating.current = true;
    setView(next);
  };
  const left = <ChevronLeft size={13} className="mr-1 shrink-0" aria-hidden="true" />;
  const right = <ChevronRight size={13} className="ml-auto shrink-0" aria-hidden="true" />;

  return <ContextMenuContent className="pet-context-menu" collisionPadding={8}>
    {view === "main" ? <>
      {groups.length > 0 && <ContextMenuItem ref={first} onSelect={go("groups")}>{t("desktopPet.actions")}{right}</ContextMenuItem>}
      <ContextMenuItem onSelect={() => void openMain().catch(onError)}>{t("desktopPet.openMain")}</ContextMenuItem>
      <ContextMenuItem onSelect={() => void updatePet({ sleeping: true }).catch(onError)}>{t("desktopPet.sleep")}</ContextMenuItem>
      <ContextMenuItem onSelect={() => void resetPet().catch(onError)}>{t("desktopPet.reset")}</ContextMenuItem>
      <ContextMenuItem disabled>{t("desktopPet.chat")}</ContextMenuItem>
    </> : <>
      <ContextMenuItem ref={first} onSelect={go(group ? "groups" : "main")}>{left}{t("desktopPet.menuBack")}</ContextMenuItem>
      <ContextMenuSeparator />
      {group ? group.names.map(name => (
        <ContextMenuItem key={name} onSelect={() => onPlay(name)}>{name}</ContextMenuItem>
      )) : groups.map(item => (
        <ContextMenuItem key={item.id} onSelect={go({ group: item.id })}>{t(item.label)}{right}</ContextMenuItem>
      ))}
    </>}
  </ContextMenuContent>;
}
