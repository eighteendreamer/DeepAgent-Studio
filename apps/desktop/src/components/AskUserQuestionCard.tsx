import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, MessageSquareText, PenLine } from "lucide-react";
import type { QuestionRequest } from "../types";
import { Input } from "./shadcn/input";
import { Panel } from "./ui/Panel";
import { TintButton } from "./ui/TintButton";

interface Props {
  request: QuestionRequest;
  /** Total queued questions (including this one). */
  queueCount?: number;
  onAnswer: (req: QuestionRequest, answers: Record<string, string>) => void;
  onDecline: (req: QuestionRequest) => void;
}

/** The free-text choice label. It is not one of the model's options. */
const OTHER = "__other__";

function selectedLabels(selection: string[] | undefined): string[] {
  return selection ?? [];
}

/**
 * A question the model asked mid-run. It floats above the composer like the
 * approval card: the run is blocked until this is answered or skipped, so the
 * options stay visible without a modal that hides the conversation.
 */
export function AskUserQuestionCard({ request, queueCount = 0, onAnswer, onDecline }: Props) {
  const { t } = useTranslation();
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const [otherText, setOtherText] = useState<Record<string, string>>({});
  const remaining = Math.max(0, queueCount - 1);

  const toggle = (question: string, value: string, multi: boolean) => {
    setPicked((prev) => {
      const current = selectedLabels(prev[question]);
      if (!multi) return { ...prev, [question]: [value] };
      const next = current.includes(value)
        ? current.filter((label) => label !== value)
        : value === OTHER
          ? [OTHER]
          : [...current.filter((label) => label !== OTHER), value];
      return { ...prev, [question]: next };
    });
  };

  const answersFor = (question: string): string | null => {
    const labels = selectedLabels(picked[question]);
    if (labels.includes(OTHER)) {
      const text = (otherText[question] ?? "").trim();
      return text || null;
    }
    return labels.length > 0 ? labels.join(", ") : null;
  };

  const complete = request.questions.every((q) => answersFor(q.question) !== null);

  const submit = () => {
    const answers: Record<string, string> = {};
    for (const q of request.questions) {
      const answer = answersFor(q.question);
      if (answer) answers[q.question] = answer;
    }
    onAnswer(request, answers);
  };

  return (
    <Panel menu={false} className="w-full overflow-hidden rounded-2xl">
      <div className="px-4 pt-3 pb-2 flex items-center gap-2">
        <MessageSquareText size={14} className="shrink-0 text-text-secondary" />
        <span className="text-[13px] font-semibold text-text-base">
          {t("askUserDialog.title")}
        </span>
        {remaining > 0 && (
          <span className="text-[10px] text-text-secondary border border-border-theme rounded-full px-2 py-0.5">
            {t("askUserDialog.pending", { count: remaining })}
          </span>
        )}
      </div>

      <div className="px-4 pb-3 flex flex-col gap-3">
        {request.questions.map((question, questionIndex) => (
          <div key={`${questionIndex}-${question.question}`} className="flex flex-col gap-1.5">
            {question.header && (
              <span className="w-fit text-[10px] font-semibold uppercase tracking-wide rounded-full px-2 py-0.5 bg-gray-100 text-gray-600">
                {question.header}
              </span>
            )}
            <p className="text-[13px] text-text-base leading-relaxed">{question.question}</p>
            <div className="flex flex-col gap-1">
              {question.options.map((option) => {
                const active = selectedLabels(picked[question.question]).includes(option.label);
                return (
                  <button
                    key={option.label}
                    type="button"
                    onClick={() => toggle(question.question, option.label, question.multi_select)}
                    className={`flex w-full min-w-0 items-start gap-2 rounded-lg border px-3 py-2 text-left transition ${
                      active
                        ? "border-primary/60 bg-primary/5"
                        : "border-border-theme bg-white hover:bg-gray-50"
                    }`}
                  >
                    <span
                      className={`mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full border ${
                        active ? "border-primary bg-primary text-white" : "border-border-theme"
                      }`}
                    >
                      {active && <Check size={11} strokeWidth={3} />}
                    </span>
                    <span className="min-w-0">
                      <span className="block text-[13px] text-text-base">{option.label}</span>
                      {option.description && (
                        <span className="block text-[12px] text-text-secondary leading-snug">
                          {option.description}
                        </span>
                      )}
                    </span>
                  </button>
                );
              })}
              <button
                type="button"
                onClick={() => toggle(question.question, OTHER, question.multi_select)}
                className={`flex w-full min-w-0 items-center gap-2 rounded-lg border px-3 py-2 text-left transition ${
                  selectedLabels(picked[question.question]).includes(OTHER)
                    ? "border-primary/60 bg-primary/5"
                    : "border-border-theme bg-white hover:bg-gray-50"
                }`}
              >
                <PenLine size={13} className="shrink-0 text-text-secondary" />
                <span className="text-[13px] text-text-base">{t("askUserDialog.other")}</span>
              </button>
              {selectedLabels(picked[question.question]).includes(OTHER) && (
                <Input
                  autoFocus
                  value={otherText[question.question] ?? ""}
                  onChange={(event) =>
                    setOtherText((prev) => ({
                      ...prev,
                      [question.question]: event.target.value,
                    }))
                  }
                  placeholder={t("askUserDialog.otherPlaceholder")}
                  className="h-9 text-[13px]"
                />
              )}
            </div>
          </div>
        ))}
      </div>

      <div className="px-4 py-2.5 border-t border-border-theme flex justify-end gap-2 bg-[#FbFcFd]">
        <TintButton
          variant="default"
          className="px-3.5 py-1.5 rounded-full text-[13px]"
          onClick={() => onDecline(request)}
        >
          {t("askUserDialog.skip")}
        </TintButton>
        <TintButton
          variant="primary"
          disabled={!complete}
          className="px-3.5 py-1.5 rounded-full text-[13px] shadow-sm"
          onClick={submit}
        >
          {t("askUserDialog.submit")}
        </TintButton>
      </div>
    </Panel>
  );
}
