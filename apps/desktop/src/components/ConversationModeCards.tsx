import { useEffect, useState } from "react";
import { ArrowRight, ListChecks, Pause, Pencil, Play, Trash2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { GoalTarget } from "./GoalTarget";
import { goalElapsedMs, type GoalPreview } from "./conversationModePreview";
import { Button } from "./shadcn/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "./shadcn/dialog";
import { Textarea } from "./shadcn/textarea";
import { HoverInfo } from "./ui/HoverInfo";

function elapsedLabel(ms: number) {
  const seconds = Math.floor(ms / 1000);
  const units = [[86400, "d"], [3600, "h"], [60, "m"], [1, "s"]] as const;
  return units.flatMap(([divisor, unit]) => {
    const value = divisor === 86400 ? Math.floor(seconds / divisor) : Math.floor(seconds / divisor) % (unit === "h" ? 24 : 60);
    return value ? [`${value}${unit}`] : [];
  }).join(" ") || "0s";
}

export function ComposerGoalBar({ goal, onToggle, onEdit, onRemove }: {
  goal: GoalPreview;
  onToggle: () => void;
  onEdit: (text: string) => void;
  onRemove: () => void;
}) {
  const { t } = useTranslation();
  const [now, setNow] = useState(Date.now);
  const [draft, setDraft] = useState<string | null>(null);
  const paused = goal.resumedAt === null;
  useEffect(() => {
    if (goal.resumedAt === null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [goal.resumedAt]);
  const pauseLabel = t(paused ? "composer.goal.resume" : "composer.goal.pause");
  const actions = [
    { label: t("composer.goal.remove"), icon: Trash2, run: onRemove },
    { label: pauseLabel, icon: paused ? Play : Pause, run: onToggle },
    { label: t("composer.goal.edit"), icon: Pencil, run: () => setDraft(goal.text) },
  ];
  const save = () => {
    const text = draft?.trim();
    if (!text) return;
    onEdit(text);
    setDraft(null);
  };
  return (
    <>
      <div data-composer-goal className="mx-3 -mb-3 flex min-w-0 items-center gap-2 rounded-t-2xl border border-border-theme bg-hover-bg px-3 pb-4 pt-1.5 text-xs text-text-secondary">
        <GoalTarget className="size-4 shrink-0" aria-hidden="true" />
        <span className="shrink-0 font-medium text-text-base">{t(paused ? "composer.goal.paused" : "composer.goal.active")}</span>
        <span className="min-w-0 flex-1 truncate">{goal.text}</span>
        <span className="shrink-0 tabular-nums">{elapsedLabel(goalElapsedMs(goal, now))}</span>
        <div className="flex shrink-0 items-center gap-0.5">
          {actions.map(({ label, icon: Icon, run }) => (
            <HoverInfo key={label} content={label}>
              <Button type="button" size="icon" variant="ghost" aria-label={label} onClick={run} className="size-6 rounded-md text-text-secondary hover:bg-hover-bg hover:text-text-base">
                <Icon className="size-3.5" aria-hidden="true" />
              </Button>
            </HoverInfo>
          ))}
        </div>
      </div>
      <Dialog open={draft !== null} onOpenChange={(open) => { if (!open) setDraft(null); }}>
        <DialogContent className="w-[min(420px,calc(100vw-32px))]">
          <DialogHeader className="px-4 py-3"><DialogTitle className="text-base">{t("composer.goal.edit")}</DialogTitle></DialogHeader>
          <div className="px-4">
            <Textarea aria-label={t("composer.goal.edit")} value={draft ?? ""} onChange={(event) => setDraft(event.currentTarget.value)} className="bg-elevated-bg" autoFocus />
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDraft(null)}>{t("composer.goal.cancel")}</Button>
            <Button disabled={!draft?.trim()} onClick={save}>{t("composer.goal.save")}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

export function PlanReviewActions({ onAccept, onDismiss, onRevise }: {
  onAccept: () => void;
  onDismiss: () => void;
  onRevise: () => void;
}) {
  const { t } = useTranslation();
  return (
    <section data-plan-review aria-label={t("composer.plan.confirm")} className="custom-scrollbar w-full max-w-4xl min-h-0 overflow-y-auto rounded-2xl border border-border-theme bg-bg-base p-2.5 shadow-[0_14px_34px_rgba(0,0,0,0.12)]">
      <div className="flex items-center justify-between gap-2 px-1.5 pb-2">
        <h3 className="flex items-center gap-2 text-sm font-medium text-text-base"><ListChecks className="size-4 text-text-secondary" aria-hidden="true" />{t("composer.plan.confirm")}</h3>
        <HoverInfo content={t("composer.plan.closeReview")}>
          <Button type="button" size="icon" variant="ghost" className="size-7" aria-label={t("composer.plan.closeReview")} onClick={onDismiss}><X className="size-4" /></Button>
        </HoverInfo>
      </div>
      <Button type="button" variant="ghost" className="w-full justify-start rounded-xl bg-hover-bg text-text-base" onClick={onAccept}>
        <Play className="size-4" aria-hidden="true" />{t("composer.plan.accept")}<ArrowRight className="ml-auto size-4 text-text-secondary" aria-hidden="true" />
      </Button>
      <div className="mt-1 flex items-center justify-between gap-2">
        <Button type="button" variant="ghost" className="min-w-0 justify-start text-text-secondary" onClick={onRevise}><Pencil className="size-4" aria-hidden="true" />{t("composer.plan.revise")}</Button>
        <Button type="button" size="sm" variant="outline" className="shrink-0 rounded-full" onClick={onDismiss}>{t("composer.plan.skip")}</Button>
      </div>
    </section>
  );
}
