declare module "react-syntax-highlighter" {
  interface SyntaxHighlighterProps {
    children: string;
    language?: string;
    style?: Record<string, import("react").CSSProperties>;
    PreTag?: string;
    customStyle?: import("react").CSSProperties;
  }

  type PrismLightComponent = import("react").ComponentType<SyntaxHighlighterProps> & {
    registerLanguage(name: string, grammar: unknown): void;
  };

  export const PrismLight: PrismLightComponent;
}

declare module "react-syntax-highlighter/dist/esm/styles/prism" {
  export const vscDarkPlus: Record<string, import("react").CSSProperties>;
}

declare module "react-syntax-highlighter/dist/esm/languages/prism/*" {
  const grammar: unknown;
  export default grammar;
}
