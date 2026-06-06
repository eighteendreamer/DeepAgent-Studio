import { memo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";
// @ts-ignore
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
// @ts-ignore
import { vscDarkPlus } from "react-syntax-highlighter/dist/esm/styles/prism";

import "katex/dist/katex.min.css"; // Ensure Katex CSS is loaded
import katex from "katex";
if (typeof window !== "undefined") {
  (window as any).katex = katex;
}
// KaTeX keeps chemistry commands such as \ce{...} in the mhchem extension.
// rehype-katex uses KaTeX under the hood, so this side-effect import must run
// before Markdown content is rendered.
import "katex/contrib/mhchem/mhchem.js";
import { EChartsBlock } from "./EChartsBlock";
import { SiteCardBlock } from "./SiteCardBlock";

interface MarkdownTextProps {
  text: string;
  tone?: "normal" | "error";
  className?: string;
  onOpenUrl?: (url: string) => void;
}

export const MarkdownText = memo(function MarkdownText({
  text,
  tone = "normal",
  className = "",
  onOpenUrl,
}: MarkdownTextProps) {
  return (
    <div
      className={`markdown-body text-[15px] leading-relaxed ${
        tone === "error" ? "text-red-500" : "text-text-base"
      } ${className} prose dark:prose-invert max-w-none`}
    >
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkMath]}
        rehypePlugins={[rehypeKatex]}
        components={{
          code({ node, inline, className, children, ...props }: any) {
            const match = /language-(\w+(?:-\w+)*)/.exec(className || "");
            const language = match ? match[1] : "";
            const content = String(children).replace(/\n$/, "");

            if (!inline && language === "echarts") {
              return <EChartsBlock content={content} />;
            }
            if (!inline && language === "site-card") {
              return <SiteCardBlock content={content} />;
            }

            return !inline ? (
              <div className="relative mb-4 mt-2 overflow-hidden rounded-lg bg-[#1e1e1e] shadow-md border border-gray-700/50">
                <div className="flex items-center justify-between bg-[#2d2d2d] px-4 py-1.5 text-xs text-gray-400">
                  <span className="font-mono lowercase">{language || "text"}</span>
                  {/* Optional: Add a copy button here later */}
                </div>
                <div className="overflow-x-auto p-4 pt-2 text-[13px]">
                  <SyntaxHighlighter
                    {...props}
                    style={vscDarkPlus as any}
                    language={language || "text"}
                    PreTag="div"
                    customStyle={{
                      margin: 0,
                      padding: 0,
                      background: "transparent",
                    }}
                  >
                    {content}
                  </SyntaxHighlighter>
                </div>
              </div>
            ) : (
              <code
                className="rounded bg-gray-100 dark:bg-gray-800 px-1.5 py-0.5 text-[0.92em] font-mono text-gray-800 dark:text-gray-200"
                {...props}
              >
                {children}
              </code>
            );
          },
          table({ children, ...props }) {
            return (
              <div className="my-4 w-full overflow-hidden rounded-lg border border-gray-200 dark:border-gray-700 shadow-sm">
                <table className="w-full text-left text-[14px] m-0" {...props}>
                  {children}
                </table>
              </div>
            );
          },
          thead({ children, ...props }) {
            return (
              <thead className="bg-[#1e293b] text-white" {...props}>
                {children}
              </thead>
            );
          },
          tbody({ children, ...props }) {
            return (
              <tbody className="divide-y divide-gray-100 dark:divide-gray-700/50 bg-transparent" {...props}>
                {children}
              </tbody>
            );
          },
          tr({ children, ...props }) {
            return (
              <tr className="bg-transparent m-0 p-0" {...props}>
                {children}
              </tr>
            );
          },
          th({ children, ...props }) {
            return (
              <th className="px-4 py-3 font-semibold first:rounded-tl-lg last:rounded-tr-lg border-0 m-0" {...props}>
                {children}
              </th>
            );
          },
          td({ children, ...props }) {
            return (
              <td className="px-4 py-3 border-0 m-0" {...props}>
                {children}
              </td>
            );
          },
          img({ node, ...props }) {
            return (
              <img
                className="my-3 max-h-[400px] w-auto rounded-xl object-contain shadow-sm border border-gray-100 dark:border-gray-700 hover:scale-[1.02] transition-transform duration-300"
                {...props}
              />
            );
          },
          a({ children, href, ...props }) {
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
          }
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
