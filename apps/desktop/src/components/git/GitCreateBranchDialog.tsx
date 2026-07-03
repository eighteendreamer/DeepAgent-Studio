import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

interface Props {
  open: boolean;
  title: string;
  label: string;
  initialValue: string;
  confirmLabel: string;
  loading?: boolean;
  error?: string | null;
  onClose: () => void;
  onConfirm: (name: string) => void | Promise<void>;
}

export function GitCreateBranchDialog({
  open,
  title,
  label,
  initialValue,
  confirmLabel,
  loading = false,
  error = null,
  onClose,
  onConfirm,
}: Props) {
  const { t } = useTranslation();
  const [value, setValue] = useState(initialValue);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setValue(initialValue);
  }, [initialValue, open]);

  useEffect(() => {
    if (!open) return;
    const frame = window.requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose, open]);

  if (!open) return null;

  const trimmed = value.trim();

  return (
    <div className="fixed inset-0 z-[120] flex items-center justify-center bg-black/20 px-4" onMouseDown={onClose}>
      <div
        className="w-full max-w-[460px] overflow-hidden rounded-2xl bg-white shadow-[0_24px_64px_rgba(15,23,42,0.18)]"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="flex items-center justify-between border-b border-border-theme px-6 py-4">
          <h3 className="text-[16px] font-semibold text-text-base">{title}</h3>
          <button
            type="button"
            className="text-gray-400 transition-colors hover:text-text-base"
            onClick={onClose}
            disabled={loading}
            aria-label={t("git.createBranchDialog.cancel")}
          >
            <FontAwesomeIcon icon={["fas", "times"]} className="text-[14px]" />
          </button>
        </div>

        <form
          className="space-y-4 px-6 py-5"
          onSubmit={(event) => {
            event.preventDefault();
            if (!trimmed || loading) return;
            void onConfirm(trimmed);
          }}
        >
          <div>
            <label className="mb-1.5 block text-[12px] font-medium text-text-base">{label}</label>
            <input
              ref={inputRef}
              type="text"
              value={value}
              onChange={(event) => setValue(event.target.value)}
              placeholder={t("git.createBranchDialog.placeholder")}
              className="w-full rounded-xl border border-border-theme bg-white px-3 py-2 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
            />
          </div>

          {error && <div className="rounded-xl bg-red-50 px-3 py-2 text-[12px] text-red-600">{error}</div>}

          <div className="flex items-center justify-end gap-3 pt-1">
            <button
              type="button"
              className="text-[13px] text-text-secondary transition-colors hover:text-text-base disabled:cursor-not-allowed disabled:opacity-60"
              onClick={onClose}
              disabled={loading}
            >
              {t("git.createBranchDialog.cancel")}
            </button>
            <button
              type="submit"
              className="inline-flex min-w-[96px] items-center justify-center rounded-full bg-black px-5 py-2 text-[13px] font-medium text-white transition-colors hover:bg-gray-800 disabled:cursor-not-allowed disabled:opacity-50"
              disabled={!trimmed || loading}
            >
              {loading ? (
                <>
                  <FontAwesomeIcon icon={["fas", "spinner"]} className="mr-2 animate-spin text-[12px]" />
                  {confirmLabel}
                </>
              ) : (
                confirmLabel
              )}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
