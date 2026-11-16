import { memo, useEffect, useState } from "react";
import { MarkdownLoading } from "./MarkdownLoading";

import {
  createMarkdownComponents,
  markdownBodyClass,
  type MarkdownTextProps,
} from "./MarkdownText.shared";

type MathMarkdownRuntime = {
  ReactMarkdown: typeof import("react-markdown").default;
  rehypeKatex: typeof import("rehype-katex").default;
  remarkGfm: typeof import("remark-gfm").default;
  remarkMath: typeof import("remark-math").default;
};

let mathRuntimePromise: Promise<MathMarkdownRuntime> | null = null;
let mathRuntime: MathMarkdownRuntime | null = null;

export function loadMathMarkdownRuntime() {
  mathRuntimePromise ??= Promise.all([
    import("react-markdown"),
    import("rehype-katex"),
    import("remark-gfm"),
    import("remark-math"),
    import("katex"),
    import("katex/dist/katex.min.css"),
  ]).then(async ([ReactMarkdownModule, rehypeKatexModule, remarkGfmModule, remarkMathModule, katexModule]) => {
    if (typeof window !== "undefined") {
      (window as Window & { katex?: unknown }).katex = katexModule.default;
    }
    await import("katex/contrib/mhchem/mhchem.js");
    mathRuntime = {
      ReactMarkdown: ReactMarkdownModule.default,
      rehypeKatex: rehypeKatexModule.default,
      remarkGfm: remarkGfmModule.default,
      remarkMath: remarkMathModule.default,
    };
    return mathRuntime;
  }).catch(error => {
    mathRuntimePromise = null;
    throw error;
  });
  return mathRuntimePromise;
}

export const MathMarkdownText = memo(function MathMarkdownText({
  text,
  tone = "normal",
  className = "",
  onOpenUrl,
}: MarkdownTextProps) {
  const [runtime, setRuntime] = useState<MathMarkdownRuntime | null>(() => mathRuntime);

  useEffect(() => {
    let cancelled = false;
    void loadMathMarkdownRuntime().then((loaded) => {
      if (!cancelled) setRuntime(loaded);
    }).catch(error => {
      if (!cancelled) console.error("math renderer preparation failed", error);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!runtime) {
    return <MarkdownLoading />;
  }

  const { ReactMarkdown, rehypeKatex, remarkGfm, remarkMath } = runtime;

  return (
    <div className={markdownBodyClass(tone, className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkMath]}
        rehypePlugins={[rehypeKatex]}
        components={createMarkdownComponents(onOpenUrl)}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
