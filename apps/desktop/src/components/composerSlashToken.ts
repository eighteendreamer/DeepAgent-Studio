// A zero-width character occupies one native textarea position. The mirror
// supplies its visible width, while the controlled draft keeps the wire text.
const SLASH_MARKER = "\u200B";

export function slashCommandKind(title: string): "skill" | "plugin" | "command" {
  if (title === "/skills") return "skill";
  // Plugin-defined commands use the /plugin-name:command namespace.
  if (title === "/plugin" || title === "/plugins" || title.includes(":")) return "plugin";
  return "command";
}

export function findCommittedSlash(draft: string, titles: ReadonlySet<string>) {
  const match = /^\/([a-zA-Z0-9_:-]+) /.exec(draft);
  if (!match) return null;
  const title = `/${match[1]}`;
  if (!titles.has(title)) return null;
  return {
    prefix: match[0],
    label: match[1][0].toUpperCase() + match[1].slice(1),
    kind: slashCommandKind(title),
  };
}

export function projectSlashDraft(draft: string, prefix: string): string {
  return prefix ? SLASH_MARKER + draft.slice(prefix.length) : draft;
}

export function toSlashEditorOffset(offset: number, prefix: string): number {
  if (!prefix || offset === 0) return offset;
  return Math.max(1, offset - prefix.length + 1);
}

export function restoreSlashDraft(editor: string, prefix: string) {
  const marker = prefix ? editor.indexOf(SLASH_MARKER) : -1;
  if (marker < 0) return { draft: editor, offset: (index: number) => index };
  return {
    draft: editor.slice(0, marker) + prefix + editor.slice(marker + 1),
    offset: (index: number) => index > marker ? index + prefix.length - 1 : index,
  };
}
