import { memo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import {
  createMarkdownComponents,
  markdownBodyClass,
  type MarkdownTextProps,
} from "./MarkdownText.shared";

export const BasicMarkdownText = memo(function BasicMarkdownText({
  text,
  tone = "normal",
  className = "",
  onOpenUrl,
}: MarkdownTextProps) {
  return (
    <div className={markdownBodyClass(tone, className)}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={createMarkdownComponents(onOpenUrl)}>
        {text}
      </ReactMarkdown>
    </div>
  );
});
