import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowRight, Check, ChevronLeft, ChevronRight, PenLine, X } from "lucide-react";
import type { QuestionRequest } from "../types";
import { Input } from "./shadcn/input";
import { TintButton } from "./ui/TintButton";

interface Props {
  request: QuestionRequest;
  /** How many question sets are queued, including this one. */
  queueCount?: number;
  onAnswer: (req: QuestionRequest, answers: Record<string, string>) => void;
  onDecline: (req: QuestionRequest) => void;
}

/** The free-text choice. It is not one of the model's options. */
const OTHER = "__other__";

/** The tool schema asks the model to suffix its preferred option with this. */
const RECOMMENDED_MARKER = /\s*[（(]\s*(recommended|推荐)\s*[)）]\s*$/i;

function optionLabel(label: string): { text: string; recommended: boolean } {
  return {
    text: label.replace(RECOMMENDED_MARKER, "").trim(),
    recommended: RECOMMENDED_MARKER.test(label),
  };
}

/**
 * One question at a time, anchored to the bottom of the window over the
 * composer: the run is blocked here, so the answer surface must not compete
 * with the transcript or hide behind it.
 */
export function AskUserQuestionCard({ request, queueCount = 1, onAnswer, onDecline }: Props) {
  const { t } = useTranslation();
  const [index, setIndex] = useState(0);
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const [otherText, setOtherText] = useState<Record<string, string>>({});

  const questions = request.questions;
  const safeIndex = Math.min(index, questions.length - 1);
  const current = questions[safeIndex];
  const choices = picked[current.question] ?? [];
  const showingOther = choices.includes(OTHER);

  const answerFor = (questionText: string): string | null => {
    const labels = picked[questionText] ?? [];
    if (labels.includes(OTHER)) {
      const text = (otherText[questionText] ?? "").trim();
      return text || null;
    }
    return labels.length > 0 ? labels.join(", ") : null;
  };

  const complete = questions.every((question) => answerFor(question.question) !== null);

  const submit = () => {
    const answers: Record<string, string> = {};
    for (const question of questions) {
      const answer = answerFor(question.question);
      if (answer) answers[question.question] = answer;
    }
    onAnswer(request, answers);
  };

  const apply = (value: string) => {
    const key = current.question;
    let next: string[];
    if (!current.multi_select) {
      next = [value];
    } else if (value === OTHER) {
      next = choices.includes(OTHER) ? [] : [OTHER];
    } else if (choices.includes(value)) {
      next = choices.filter((label) => label !== value);
    } else {
      next = [...choices.filter((label) => label !== OTHER), value];
    }
    setPicked((prev) => ({ ...prev, [key]: next }));
    // A single-choice answer moves on by itself; a multi-choice one waits until
    // the user pages forward, otherwise every tap would jump away.
    const answered = next.length > 0 && (value !== OTHER || (otherText[key] ?? "").trim());
    if (answered && !current.multi_select && safeIndex < questions.length - 1) {
      setIndex(safeIndex + 1);
    }
  };

  return (
    <div className="w-full max-w-4xl rounded-2xl border border-border-theme bg-bg-base px-4 pb-3.5 pt-3 shadow-[0_14px_34px_rgba(0,0,0,0.12)]">
      <div className="flex flex-col gap-2">
        <div className="flex items-start gap-3">
          <p className="min-w-0 flex-1 text-[13.5px] font-medium leading-6 text-text-base">
            {current.question}
            {current.multi_select && !current.question.includes("多选") && (
              <span className="ml-1.5 text-[11px] font-normal text-text-secondary">
                {t("askUserDialog.multiHint")}
              </span>
            )}
          </p>
          {queueCount > 1 && (
            <span className="mt-1 shrink-0 rounded-full border border-border-theme px-2 py-0.5 text-[10px] text-text-secondary">
              {t("askUserDialog.pending", { count: queueCount - 1 })}
            </span>
          )}
          <div className="flex shrink-0 items-center gap-0.5 text-text-secondary">
            <button
              type="button"
              disabled={safeIndex === 0}
              onClick={() => setIndex(Math.max(0, safeIndex - 1))}
              className="flex h-6 w-6 items-center justify-center rounded-md transition hover:bg-hover-bg disabled:opacity-35 disabled:hover:bg-transparent"
              aria-label={t("askUserDialog.previous")}
            >
              <ChevronLeft size={15} />
            </button>
            <span className="min-w-[52px] text-center text-[12px] tabular-nums">
              {t("askUserDialog.of", { current: safeIndex + 1, total: questions.length })}
            </span>
            <button
              type="button"
              disabled={safeIndex >= questions.length - 1}
              onClick={() => setIndex(Math.min(questions.length - 1, safeIndex + 1))}
              className="flex h-6 w-6 items-center justify-center rounded-md transition hover:bg-hover-bg disabled:opacity-35 disabled:hover:bg-transparent"
              aria-label={t("askUserDialog.next")}
            >
              <ChevronRight size={15} />
            </button>
            <button
              type="button"
              onClick={() => onDecline(request)}
              className="ml-1 flex h-6 w-6 items-center justify-center rounded-md transition hover:bg-hover-bg"
              aria-label={t("askUserDialog.skip")}
            >
              <X size={15} />
            </button>
          </div>
        </div>

        <div className="flex max-h-[38vh] min-h-0 flex-col gap-1 overflow-y-auto">
          {current.options.map((option, optionIndex) => {
            const selected = choices.includes(option.label);
            const display = optionLabel(option.label);
            return (
              <button
                key={option.label}
                type="button"
                onClick={() => apply(option.label)}
                className={`flex w-full min-w-0 items-start gap-2.5 rounded-lg px-2.5 py-2 text-left transition ${
                  selected ? "bg-hover-bg" : "hover:bg-hover-bg/60"
                }`}
              >
                <span
                  className={`mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded border text-[11px] tabular-nums ${
                    selected
                      ? "border-primary bg-primary text-white"
                      : "border-border-theme text-text-secondary"
                  }`}
                >
                  {selected ? <Check size={12} strokeWidth={3} /> : optionIndex + 1}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="flex items-center gap-1.5">
                    <span className="text-[13px] text-text-base">{display.text}</span>
                    {display.recommended && (
                      <span className="rounded border border-border-theme px-1.5 py-px text-[10px] text-text-secondary">
                        {t("askUserDialog.recommended")}
                      </span>
                    )}
                  </span>
                  {option.description && (
                    <span className="block text-[12px] leading-snug text-text-secondary">
                      {option.description}
                    </span>
                  )}
                </span>
                {selected && <ArrowRight size={14} className="mt-1 shrink-0 text-text-secondary" />}
              </button>
            );
          })}

          <div className="flex flex-col gap-1">
            <button
              type="button"
              onClick={() => apply(OTHER)}
              className={`flex w-full min-w-0 items-center gap-2.5 rounded-lg px-2.5 py-2 text-left transition ${
                showingOther ? "bg-hover-bg" : "hover:bg-hover-bg/60"
              }`}
            >
              <PenLine size={14} className="shrink-0 text-text-secondary" />
              <span className="text-[13px] text-text-base">{t("askUserDialog.other")}</span>
            </button>
            {showingOther && (
              <Input
                autoFocus
                value={otherText[current.question] ?? ""}
                onChange={(event) =>
                  setOtherText((prev) => ({ ...prev, [current.question]: event.target.value }))
                }
                onKeyDown={(event) => {
                  if (event.key !== "Enter" || !(otherText[current.question] ?? "").trim()) return;
                  if (safeIndex < questions.length - 1) setIndex(safeIndex + 1);
                  else if (complete) submit();
                }}
                placeholder={t("askUserDialog.otherPlaceholder")}
                className="h-9 text-[13px]"
              />
            )}
          </div>
        </div>

        <div className="flex items-center justify-end gap-2 pt-1">
          <TintButton
            variant="default"
            className="px-3.5 py-1.5 text-[13px]"
            onClick={() => onDecline(request)}
          >
            {t("askUserDialog.skip")}
          </TintButton>
          <TintButton
            variant="primary"
            disabled={!complete}
            className="px-3.5 py-1.5 text-[13px] disabled:cursor-not-allowed disabled:opacity-40"
            onClick={submit}
          >
            {t("askUserDialog.submit")}
          </TintButton>
        </div>
      </div>
    </div>
  );
}
