import type { ThemeBackgroundImage, ThemePreferences } from "./themeTypes";

export const BACKGROUND_IMAGE_ACCEPT = "image/png,image/jpeg,image/webp";
export const MAX_BACKGROUND_FILE_BYTES = 10 * 1024 * 1024;
export const MAX_BACKGROUND_DATA_URL_LENGTH = 1024 * 1024;
export const DEFAULT_BACKGROUND_TRANSPARENCY = 75;

type ImageErrorCode = "unsupported" | "tooLarge" | "decode" | "encode";

export class BackgroundImageError extends Error {
  constructor(readonly code: ImageErrorCode) {
    super(`Background image: ${code}`);
    this.name = "BackgroundImageError";
  }
}

export function normalizeBackgroundTransparency(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.round(Math.max(0, Math.min(100, value)))
    : DEFAULT_BACKGROUND_TRANSPARENCY;
}

export function normalizeBackgroundImage(value: unknown): ThemeBackgroundImage | null {
  if (!value || typeof value !== "object") return null;
  const image = value as Record<string, unknown>;
  if (
    typeof image.dataUrl !== "string" ||
    image.dataUrl.length > MAX_BACKGROUND_DATA_URL_LENGTH ||
    !/^data:image\/(?:png|jpeg|webp);base64,[A-Za-z0-9+/]+={0,2}$/.test(image.dataUrl)
  ) return null;
  return {
    dataUrl: image.dataUrl,
    name: typeof image.name === "string"
      ? image.name.replace(/[\u0000-\u001f\u007f]/g, "").slice(0, 255)
      : "",
  };
}

export function previewBackgroundTransparency(value: number): void {
  document.documentElement.style.setProperty(
    "--app-background-opacity",
    String(1 - normalizeBackgroundTransparency(value) / 100),
  );
}

/** Shared by startup and the provider; never loads remote or arbitrary CSS URLs. */
export function applyThemeBackground(preferences: ThemePreferences): boolean {
  const root = document.documentElement;
  // Canvas has its own opaque workspace. Keep the shared preference intact so
  // its updates still reach ordinary application windows through the provider.
  const canvasWindow = new URLSearchParams(window.location.hash.slice(1)).get("window") === "canvas";
  const image = canvasWindow ? null : normalizeBackgroundImage(preferences.backgroundImage);
  root.classList.toggle("theme-custom-background", image !== null);
  if (image) root.style.setProperty("--app-background-image", `url("${image.dataUrl}")`);
  else root.style.removeProperty("--app-background-image");
  previewBackgroundTransparency(preferences.backgroundTransparency);
  const translucentWindow = preferences.translucentSidebar && !image && !canvasWindow;
  root.classList.toggle("theme-translucent-sidebar", translucentWindow);
  return translucentWindow;
}

/** Decode and re-encode locally, bounding both display dimensions and storage size. */
export async function prepareBackgroundImage(file: File): Promise<ThemeBackgroundImage> {
  if (!BACKGROUND_IMAGE_ACCEPT.split(",").includes(file.type)) {
    throw new BackgroundImageError("unsupported");
  }
  if (file.size === 0 || file.size > MAX_BACKGROUND_FILE_BYTES) {
    throw new BackgroundImageError("tooLarge");
  }
  const url = URL.createObjectURL(file);
  try {
    const image = new Image();
    await new Promise<void>((resolve, reject) => {
      image.onload = () => resolve();
      image.onerror = () => reject(new BackgroundImageError("decode"));
      image.src = url;
    });
    if (!image.naturalWidth || !image.naturalHeight ||
      image.naturalWidth * image.naturalHeight > 40_000_000) {
      throw new BackgroundImageError("decode");
    }
    const canvas = document.createElement("canvas");
    const context = canvas.getContext("2d");
    if (!context) throw new BackgroundImageError("encode");
    for (const maxEdge of [2560, 1920, 1280]) {
      const scale = Math.min(1, maxEdge / Math.max(image.naturalWidth, image.naturalHeight));
      canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
      canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
      context.drawImage(image, 0, 0, canvas.width, canvas.height);
      const result = normalizeBackgroundImage({
        dataUrl: canvas.toDataURL("image/webp", 0.82), name: file.name,
      });
      if (result) return result;
    }
    throw new BackgroundImageError("encode");
  } finally {
    URL.revokeObjectURL(url);
  }
}
