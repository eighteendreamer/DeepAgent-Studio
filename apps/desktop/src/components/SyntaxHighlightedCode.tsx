import { PrismLight as SyntaxHighlighter } from "react-syntax-highlighter";
import { vscDarkPlus } from "react-syntax-highlighter/dist/esm/styles/prism";
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

export function SyntaxHighlightedCode({ language, content }: { language: string; content: string }) {
  return (
    <SyntaxHighlighter
      style={vscDarkPlus}
      language={language || "text"}
      PreTag="div"
      customStyle={{ margin: 0, padding: 0, background: "transparent" }}
    >
      {content}
    </SyntaxHighlighter>
  );
}
