import type { ReactNode } from "react";

interface MarkdownTextProps {
  text: string;
  tone?: "normal" | "error";
  className?: string;
}

function parseInline(text: string, prefix = "i"): ReactNode[] {
  const nodes: ReactNode[] = [];
  const pattern = /(`[^`]+`|\*\*[^*]+?\*\*|\[[^\]]+?\]\([^)]+?\))/g;
  let last = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text))) {
    if (match.index > last) nodes.push(text.slice(last, match.index));
    const token = match[0];
    const key = `${prefix}-${match.index}`;

    if (token.startsWith("`")) {
      nodes.push(
        <code key={key} className="rounded bg-gray-100 px-1 py-0.5 text-[0.92em] font-mono">
          {token.slice(1, -1)}
        </code>
      );
    } else if (token.startsWith("**")) {
      nodes.push(
        <strong key={key} className="font-semibold text-text-base">
          {parseInline(token.slice(2, -2), key)}
        </strong>
      );
    } else {
      const end = token.lastIndexOf("](");
      const label = token.slice(1, end);
      const href = token.slice(end + 2, -1);
      const safeHref = /^https?:\/\//i.test(href) ? href : undefined;
      nodes.push(
        safeHref ? (
          <a
            key={key}
            href={safeHref}
            target="_blank"
            rel="noreferrer"
            className="text-blue-600 underline underline-offset-2"
          >
            {parseInline(label, key)}
          </a>
        ) : (
          token
        )
      );
    }

    last = pattern.lastIndex;
  }

  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

function renderInlineLines(lines: string[], prefix: string): ReactNode[] {
  return lines.flatMap((line, index) => {
    const parsed = parseInline(line, `${prefix}-${index}`);
    return index === lines.length - 1 ? parsed : [...parsed, <br key={`${prefix}-br-${index}`} />];
  });
}

function renderMarkdownBlock(text: string, prefix: string): ReactNode[] {
  const out: ReactNode[] = [];
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  let paragraph: string[] = [];

  const flushParagraph = () => {
    if (paragraph.length === 0) return;
    const key = `${prefix}-p-${out.length}`;
    out.push(
      <p key={key} className="mb-2 last:mb-0">
        {renderInlineLines(paragraph, key)}
      </p>
    );
    paragraph = [];
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const trimmed = line.trim();

    if (!trimmed) {
      flushParagraph();
      continue;
    }

    const heading = /^(#{1,3})\s+(.+)$/.exec(trimmed);
    if (heading) {
      flushParagraph();
      const level = heading[1].length;
      const Tag = `h${level + 2}` as keyof JSX.IntrinsicElements;
      out.push(
        <Tag key={`${prefix}-h-${i}`} className="mb-2 mt-3 first:mt-0 font-semibold text-text-base">
          {parseInline(heading[2], `${prefix}-h-${i}`)}
        </Tag>
      );
      continue;
    }

    const unordered = /^[-*]\s+(.+)$/.exec(trimmed);
    if (unordered) {
      flushParagraph();
      const items: string[] = [];
      while (i < lines.length) {
        const item = /^[-*]\s+(.+)$/.exec(lines[i].trim());
        if (!item) break;
        items.push(item[1]);
        i++;
      }
      i--;
      out.push(
        <ul key={`${prefix}-ul-${i}`} className="mb-2 list-disc pl-5 last:mb-0">
          {items.map((item, idx) => (
            <li key={idx}>{parseInline(item, `${prefix}-ul-${i}-${idx}`)}</li>
          ))}
        </ul>
      );
      continue;
    }

    const ordered = /^\d+[.)]\s+(.+)$/.exec(trimmed);
    if (ordered) {
      flushParagraph();
      const items: string[] = [];
      while (i < lines.length) {
        const item = /^\d+[.)]\s+(.+)$/.exec(lines[i].trim());
        if (!item) break;
        items.push(item[1]);
        i++;
      }
      i--;
      out.push(
        <ol key={`${prefix}-ol-${i}`} className="mb-2 list-decimal pl-5 last:mb-0">
          {items.map((item, idx) => (
            <li key={idx}>{parseInline(item, `${prefix}-ol-${i}-${idx}`)}</li>
          ))}
        </ol>
      );
      continue;
    }

    paragraph.push(line);
  }

  flushParagraph();
  return out;
}

export function MarkdownText({ text, tone = "normal", className = "" }: MarkdownTextProps) {
  const nodes: ReactNode[] = [];
  const pattern = /```([^\n`]*)\n([\s\S]*?)```/g;
  let last = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text))) {
    if (match.index > last) {
      nodes.push(...renderMarkdownBlock(text.slice(last, match.index), `md-${nodes.length}`));
    }
    nodes.push(
      <pre
        key={`code-${match.index}`}
        className="mb-2 overflow-x-auto rounded-lg bg-gray-950 px-3 py-2 text-[12px] leading-relaxed text-gray-100 last:mb-0"
      >
        <code>{match[2]}</code>
      </pre>
    );
    last = pattern.lastIndex;
  }

  if (last < text.length) {
    nodes.push(...renderMarkdownBlock(text.slice(last), `md-${nodes.length}`));
  }

  return (
    <div
      className={`text-[15px] leading-relaxed ${
        tone === "error" ? "text-red-500" : "text-text-base"
      } ${className}`}
    >
      {nodes.length > 0 ? nodes : text}
    </div>
  );
}
