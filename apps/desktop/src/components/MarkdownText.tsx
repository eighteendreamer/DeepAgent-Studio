import { lazy, memo, Suspense } from "react";
import type { MarkdownTextProps } from "./MarkdownText.shared";
import { MarkdownLoading } from "./MarkdownLoading";

const loadMathMarkdown = () => import("./MathMarkdownText").then(async (module) => {
  await module.loadMathMarkdownRuntime();
  return { default: module.MathMarkdownText };
});
const loadBasicMarkdown = () => import("./BasicMarkdownText").then((module) => ({ default: module.BasicMarkdownText }));
const MathMarkdownText = lazy(loadMathMarkdown);
const BasicMarkdownText = lazy(loadBasicMarkdown);

const BLOCK_MATH_PATTERN = /(^|\n)\s*\$\$|\\\[|\\begin\{/;
const INLINE_MATH_PATTERN = /(^|[\s([{])\$[^$\n]{1,240}\$($|[\s)\]},.;:!?])/;
const KATEX_EXTENSION_PATTERN = /\\\(|\\ce\{/;
const RICH_MARKDOWN_PATTERN =
  /(^|\n)\s{0,3}(#{1,6}\s|[-*+]\s|\d+\.\s|>\s|```|~~~)|[`*_~|]|\[[^\]]+\]\([^)]+\)|!\[[^\]]*]\([^)]+\)|https?:\/\//i;

function markdownBodyClass(tone: MarkdownTextProps["tone"], className: string) {
  return `markdown-body text-[15px] leading-relaxed ${
    tone === "error" ? "text-red-500" : "text-text-base"
  } ${className} prose dark:prose-invert max-w-none`;
}

function containsMath(text: string): boolean {
  return (
    BLOCK_MATH_PATTERN.test(text) ||
    INLINE_MATH_PATTERN.test(text) ||
    KATEX_EXTENSION_PATTERN.test(text)
  );
}

function containsRichMarkdown(text: string): boolean {
  return RICH_MARKDOWN_PATTERN.test(text);
}

/** Warm only renderers required by the loaded transcript, not every rich asset. */
export async function preloadMarkdownRenderers(texts: string[]): Promise<void> {
  const pending: Promise<unknown>[] = [];
  if (texts.some(containsMath)) pending.push(loadMathMarkdown());
  if (texts.some(text => !containsMath(text) && containsRichMarkdown(text))) pending.push(loadBasicMarkdown());
  await Promise.all(pending);
}

export const MarkdownText = memo(function MarkdownText({
  text,
  tone = "normal",
  className = "",
  onOpenUrl,
}: MarkdownTextProps) {
  if (containsMath(text)) {
    return (
      <Suspense fallback={<MarkdownLoading />}>
        <MathMarkdownText text={text} tone={tone} className={className} onOpenUrl={onOpenUrl} />
      </Suspense>
    );
  }

  if (containsRichMarkdown(text)) {
    return (
      <Suspense fallback={<MarkdownLoading />}>
        <BasicMarkdownText text={text} tone={tone} className={className} onOpenUrl={onOpenUrl} />
      </Suspense>
    );
  }

  return <PlainMarkdownText text={text} tone={tone} className={className} />;
});

const PlainMarkdownText = memo(function PlainMarkdownText({
  text,
  tone = "normal",
  className = "",
}: MarkdownTextProps) {
  return (
    <div className={markdownBodyClass(tone, className)}>
      <span className="whitespace-pre-wrap break-words">{text}</span>
    </div>
  );
});
