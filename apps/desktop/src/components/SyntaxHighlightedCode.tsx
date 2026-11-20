import type { ComponentPropsWithoutRef } from "react";
import { PrismLight as SyntaxHighlighter } from "react-syntax-highlighter";
import { vs, vscDarkPlus } from "react-syntax-highlighter/dist/esm/styles/prism";
import { useThemeContext } from "../theme/ThemeProvider";
import bash from "react-syntax-highlighter/dist/esm/languages/prism/bash";
import c from "react-syntax-highlighter/dist/esm/languages/prism/c";
import cpp from "react-syntax-highlighter/dist/esm/languages/prism/cpp";
import css from "react-syntax-highlighter/dist/esm/languages/prism/css";
import go from "react-syntax-highlighter/dist/esm/languages/prism/go";
import java from "react-syntax-highlighter/dist/esm/languages/prism/java";
import javascript from "react-syntax-highlighter/dist/esm/languages/prism/javascript";
import json from "react-syntax-highlighter/dist/esm/languages/prism/json";
import jsx from "react-syntax-highlighter/dist/esm/languages/prism/jsx";
import markup from "react-syntax-highlighter/dist/esm/languages/prism/markup";
import python from "react-syntax-highlighter/dist/esm/languages/prism/python";
import rust from "react-syntax-highlighter/dist/esm/languages/prism/rust";
import sql from "react-syntax-highlighter/dist/esm/languages/prism/sql";
import tsx from "react-syntax-highlighter/dist/esm/languages/prism/tsx";
import typescript from "react-syntax-highlighter/dist/esm/languages/prism/typescript";

const LANGUAGES = {
  bash,
  shell: bash,
  sh: bash,
  c,
  cpp,
  css,
  go,
  java,
  javascript,
  js: javascript,
  json,
  jsx,
  html: markup,
  markup,
  python,
  py: python,
  rust,
  rs: rust,
  sql,
  tsx,
  typescript,
  ts: typescript,
};

Object.entries(LANGUAGES).forEach(([name, grammar]) => {
  SyntaxHighlighter.registerLanguage(name, grammar);
});

const THEMES = {
  dark: vscDarkPlus,
  light: vs,
} as const;

export type CodeTheme = keyof typeof THEMES;

/** Follows the active app theme unless a caller pins an explicit theme
 *  (the remote file preview pins `light` because it renders on a white pane). */
function useActiveCodeTheme(override?: CodeTheme): CodeTheme {
  const { activeIsDark } = useThemeContext();
  if (override) return override;
  return activeIsDark ? "dark" : "light";
}

function ThemedCodeTag({ style, ...props }: ComponentPropsWithoutRef<"code">) {
  return <code {...props} style={{ ...style, fontFamily: "inherit" }} />;
}

export function SyntaxHighlightedCode({
  language,
  content,
  theme,
}: {
  language: string;
  content: string;
  theme?: CodeTheme;
}) {
  const activeTheme = useActiveCodeTheme(theme);
  return (
    <SyntaxHighlighter
      style={THEMES[activeTheme]}
      language={language || "text"}
      PreTag="div"
      CodeTag={ThemedCodeTag}
      customStyle={{ margin: 0, padding: 0, background: "transparent", fontFamily: "var(--code-font, monospace)" }}
    >
      {content}
    </SyntaxHighlighter>
  );
}
