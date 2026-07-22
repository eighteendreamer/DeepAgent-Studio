import { lazy, Suspense } from "react";

import { SiteCardBlock } from "./SiteCardBlock";
import { SlashPanelBlock } from "./SlashPanelBlock";

const EChartsBlock = lazy(() =>
  import("./EChartsBlock").then((module) => ({ default: module.EChartsBlock })),
);
const SyntaxHighlightedCode = lazy(() =>
  import("./SyntaxHighlightedCode").then((module) => ({ default: module.SyntaxHighlightedCode })),
);

export interface MarkdownTextProps {
  text: string;
  tone?: "normal" | "error";
  className?: string;
  onOpenUrl?: (url: string) => void;
}

export function markdownBodyClass(tone: MarkdownTextProps["tone"], className: string) {
  return `markdown-body text-[15px] leading-relaxed ${
    tone === "error" ? "text-red-500" : "text-text-base"
  } ${className} prose dark:prose-invert max-w-none`;
}

export function createMarkdownComponents(onOpenUrl?: (url: string) => void) {
  return {
    code({ node, inline, className, children, ...props }: any) {
      const match = /language-(\w+(?:-\w+)*)/.exec(className || "");
      const language = match ? match[1] : "";
      const content = String(children).replace(/\n$/, "");
      const startLine = node?.position?.start?.line;
      const endLine = node?.position?.end?.line;
      const isInlineCode =
        inline === true ||
        (!language &&
          !content.includes("\n") &&
          typeof startLine === "number" &&
          typeof endLine === "number" &&
          startLine === endLine);

      if (!isInlineCode && language === "echarts") {
        return (
          <Suspense fallback={<div className="my-3 h-[350px] w-full animate-pulse rounded-lg bg-gray-100" />}>
            <EChartsBlock content={content} />
          </Suspense>
        );
      }
      if (!isInlineCode && language === "site-card") {
        return <SiteCardBlock content={content} />;
      }
      if (!isInlineCode && language === "slash-panel") {
        return <SlashPanelBlock content={content} />;
      }

      return !isInlineCode ? (
        <div className="relative mb-4 mt-2 overflow-hidden rounded-lg bg-[#1e1e1e] shadow-md border border-gray-700/50">
          <div className="flex items-center justify-between bg-[#2d2d2d] px-4 py-1.5 text-xs text-gray-400">
            <span className="font-mono lowercase">{language || "text"}</span>
          </div>
          <div className="overflow-x-auto p-4 pt-2 text-[13px]">
            <Suspense
              fallback={
                <pre className="m-0 whitespace-pre-wrap bg-transparent p-0 font-mono text-gray-100">
                  {content}
                </pre>
              }
            >
              <SyntaxHighlightedCode language={language} content={content} />
            </Suspense>
          </div>
        </div>
      ) : (
        <code
          className="rounded-md border border-slate-200 bg-slate-50 px-1.5 py-0.5 text-[0.92em] font-mono text-slate-700 dark:border-slate-600 dark:bg-slate-800/40 dark:text-slate-100"
          {...props}
        >
          {children}
        </code>
      );
    },
    table({ children, ...props }: any) {
      return (
        <div className="my-4 w-full overflow-hidden rounded-lg border border-gray-200 dark:border-gray-700 shadow-sm">
          <table className="w-full text-left text-[14px] m-0" {...props}>
            {children}
          </table>
        </div>
      );
    },
    thead({ children, ...props }: any) {
      return (
        <thead className="bg-[#1e293b] text-white" {...props}>
          {children}
        </thead>
      );
    },
    tbody({ children, ...props }: any) {
      return (
        <tbody className="divide-y divide-gray-100 dark:divide-gray-700/50 bg-transparent" {...props}>
          {children}
        </tbody>
      );
    },
    tr({ children, ...props }: any) {
      return (
        <tr className="bg-transparent m-0 p-0" {...props}>
          {children}
        </tr>
      );
    },
    th({ children, ...props }: any) {
      return (
        <th className="px-4 py-3 font-semibold first:rounded-tl-lg last:rounded-tr-lg border-0 m-0" {...props}>
          {children}
        </th>
      );
    },
    td({ children, ...props }: any) {
      return (
        <td className="px-4 py-3 border-0 m-0" {...props}>
          {children}
        </td>
      );
    },
    img({ node, ...props }: any) {
      return (
        <img
          className="my-3 max-h-[400px] w-auto rounded-xl object-contain shadow-sm border border-gray-100 dark:border-gray-700 hover:scale-[1.02] transition-transform duration-300"
          {...props}
        />
      );
    },
    a({ children, href, ...props }: any) {
      const isWebUrl = typeof href === "string" && /^https?:\/\//i.test(href);
      return (
        <a
          href={href}
          target={onOpenUrl && isWebUrl ? undefined : "_blank"}
          rel="noreferrer"
          className="text-blue-600 dark:text-blue-400 hover:underline"
          {...props}
          onClick={(event) => {
            if (!onOpenUrl || !isWebUrl || !href) return;
            event.preventDefault();
            onOpenUrl(href);
          }}
        >
          {children}
        </a>
      );
    },
  };
}
