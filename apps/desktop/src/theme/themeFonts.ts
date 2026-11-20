type FontOption = { id: string; label: string; family: string | null };

export const FONT_OPTIONS = {
  ui: [
    { id: "system", label: "", family: null },
    { id: "inter", label: "Inter", family: "DeepAgent Inter" },
    { id: "source-sans-3", label: "Source Sans 3", family: "DeepAgent Source Sans 3" },
    { id: "carlito", label: "Carlito", family: "DeepAgent Carlito" },
    { id: "source-han-sans", label: "思源黑体 CN", family: "DeepAgent Source Han Sans CN" },
  ],
  code: [
    { id: "system", label: "", family: null },
    { id: "cascadia-mono", label: "Cascadia Mono", family: "DeepAgent Cascadia Mono" },
    { id: "jetbrains-mono", label: "JetBrains Mono", family: "DeepAgent JetBrains Mono" },
    { id: "fira-code", label: "Fira Code", family: "DeepAgent Fira Code" },
  ],
} as const satisfies Record<string, readonly FontOption[]>;

export type FontKind = keyof typeof FONT_OPTIONS;
type CatalogOption<K extends FontKind> = (typeof FONT_OPTIONS)[K][number];
export type FontId<K extends FontKind> = CatalogOption<K>["id"];

const LEGACY_FONTS = {
  ui: 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto',
  code: 'ui-monospace, "SFMono-Regular", Menlo, Monaco, Consolas',
};
const DEFAULT_STACKS = {
  ui: `${LEGACY_FONTS.ui}, "Helvetica Neue", Arial, sans-serif`,
  code: `${LEGACY_FONTS.code}, "Liberation Mono", "Courier New", monospace`,
};

export function resolveFont<K extends FontKind>(kind: K, value: unknown): CatalogOption<K> {
  const options: readonly FontOption[] = FONT_OPTIONS[kind];
  const saved = typeof value === "string" ? value.trim().toLowerCase() : "";
  const firstFamily = saved.split(",", 1)[0].trim().replace(/^["']|["']$/g, "");
  // The legacy default stack starts with Inter but meant system fallback.
  const match = saved === LEGACY_FONTS[kind].toLowerCase() ? undefined : options.find(
    option => option.id === saved || (option.family !== null && (
      option.family.toLowerCase() === firstFamily || option.label.toLowerCase() === firstFamily
      || (option.id === "source-han-sans" && ["source han sans sc", "source han sans cn", "思源黑体"].includes(firstFamily))
    )),
  );
  return (match ?? FONT_OPTIONS[kind][0]) as CatalogOption<K>;
}

export function fontStack(kind: FontKind, value: unknown): string {
  const { family } = resolveFont(kind, value);
  return family === null ? DEFAULT_STACKS[kind] : `"${family}", ${DEFAULT_STACKS[kind]}`;
}
