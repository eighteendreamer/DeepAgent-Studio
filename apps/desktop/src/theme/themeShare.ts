import { parseHex } from "./colorUtils";
import type { ThemePalette, ThemeVariant } from "./themeTypes";

const SHARE_PREFIX = "deepagent-theme:v1:";
const MAX_SHARE_LENGTH = 8192;
const MAX_NAME_LENGTH = 60;

export interface ThemeSharePayload {
  schemaVersion: 1;
  name: string;
  variant: ThemeVariant;
  palette: ThemePalette;
}

export interface ThemeParseResult {
  ok: boolean;
  payload?: ThemeSharePayload;
  error?: string;
}

function base64UrlEncode(input: string): string {
  const bytes = new TextEncoder().encode(input);
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function base64UrlDecode(input: string): string | null {
  try {
    const b64 = input.replace(/-/g, "+").replace(/_/g, "/");
    const padded = b64 + "=".repeat((4 - (b64.length % 4)) % 4);
    const binary = atob(padded);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return new TextDecoder().decode(bytes);
  } catch {
    return null;
  }
}

const PALETTE_COLOR_KEYS: Array<keyof ThemePalette> = [
  "accent",
  "accentHover",
  "background",
  "sidebar",
  "elevated",
  "foreground",
  "foregroundMuted",
  "border",
  "hover",
  "selection",
];

function validatePalette(raw: unknown): ThemePalette | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const o = raw as Record<string, unknown>;
  const result = {} as Record<string, unknown>;
  for (const key of PALETTE_COLOR_KEYS) {
    const v = o[key];
    if (typeof v !== "string" || !parseHex(v)) return null;
    result[key] = v.toUpperCase();
  }
  const contrast = o.contrast;
  if (
    typeof contrast !== "number" ||
    !Number.isFinite(contrast) ||
    contrast < 0 ||
    contrast > 100
  ) {
    return null;
  }
  result.contrast = Math.round(contrast);
  return result as unknown as ThemePalette;
}

export function encodeThemeShare(payload: ThemeSharePayload): string {
  const json = JSON.stringify({
    schemaVersion: 1,
    name: payload.name.slice(0, MAX_NAME_LENGTH),
    variant: payload.variant,
    palette: payload.palette,
  });
  return SHARE_PREFIX + base64UrlEncode(json);
}

export function decodeThemeShare(value: string): ThemeParseResult {
  const trimmed = value.trim();
  if (trimmed.length > MAX_SHARE_LENGTH) {
    return { ok: false, error: "too_long" };
  }
  if (!trimmed.startsWith(SHARE_PREFIX)) {
    return { ok: false, error: "invalid_prefix" };
  }
  const json = base64UrlDecode(trimmed.slice(SHARE_PREFIX.length));
  if (!json) return { ok: false, error: "invalid_encoding" };

  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return { ok: false, error: "invalid_json" };
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    return { ok: false, error: "invalid_schema" };
  }
  const o = raw as Record<string, unknown>;
  if (o.schemaVersion !== 1) return { ok: false, error: "invalid_schema" };
  if (o.variant !== "light" && o.variant !== "dark") {
    return { ok: false, error: "invalid_variant" };
  }
  if (typeof o.name !== "string" || !o.name.trim()) {
    return { ok: false, error: "invalid_name" };
  }
  const name = o.name.trim().slice(0, MAX_NAME_LENGTH);
  // Reject names containing control chars or markup.
  if (/[<>\u0000-\u001F\u007F]/.test(name)) {
    return { ok: false, error: "invalid_name" };
  }
  const palette = validatePalette(o.palette);
  if (!palette) return { ok: false, error: "invalid_palette" };

  return {
    ok: true,
    payload: { schemaVersion: 1, name, variant: o.variant, palette },
  };
}
