import { invoke } from "@tauri-apps/api/core";
import { importCanvasMedia } from "./canvasMedia";

/**
 * Canvas preference documents in the kernel database.
 *
 * The creative library, the snippet library and the workspace output folders
 * used to live in `localStorage`, so they died with the webview profile and
 * could not be read by anything else in the product. Values are cached in
 * memory after one hydration pass, which keeps the existing synchronous call
 * sites synchronous; writes are merged and flushed to the backend.
 *
 * Legacy `localStorage` payloads are read once, upgraded (inline base64 images
 * become artifacts) and then deleted, so the same canvas data never has two
 * homes.
 */

export type CanvasPreferenceKey = "creative-library" | "workflow-snippets" | "workspace-dirs";

export const CANVAS_PREFERENCE_KEYS: CanvasPreferenceKey[] = [
  "creative-library",
  "workflow-snippets",
  "workspace-dirs",
];

/** Keys written by the pre-database canvas. */
const LEGACY_STORAGE_KEYS: Record<CanvasPreferenceKey, string> = {
  "creative-library": "canvas-creative-library",
  "workflow-snippets": "workflow-canvas-professional-snippets",
  "workspace-dirs": "workflow-workspace",
};

/** 连续编辑合并落库。 */
const FLUSH_DEBOUNCE_MS = 400;

const cache = new Map<CanvasPreferenceKey, unknown>();
const dirty = new Set<CanvasPreferenceKey>();
const timers = new Map<CanvasPreferenceKey, ReturnType<typeof setTimeout>>();

let hydration: Promise<void> | null = null;

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function readLegacy(key: CanvasPreferenceKey): unknown {
  try {
    const raw = localStorage.getItem(LEGACY_STORAGE_KEYS[key]);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

function forgetLegacy(key: CanvasPreferenceKey) {
  try {
    localStorage.removeItem(LEGACY_STORAGE_KEYS[key]);
  } catch {
    // 存储不可用不影响：数据库已经是唯一读取来源
  }
}

interface LegacyLibraryItem {
  imageUrl?: string;
  [extra: string]: unknown;
}

/** 旧库里的图片还是内联 base64，先转成制品再落库。 */
async function upgradeLibraryImages(value: unknown): Promise<unknown> {
  if (!Array.isArray(value)) return value;
  const items: unknown[] = [];
  for (const item of value) {
    const entry = item as LegacyLibraryItem;
    if (typeof entry?.imageUrl === "string" && entry.imageUrl.startsWith("data:")) {
      try {
        items.push({ ...entry, imageUrl: await importCanvasMedia("image", { dataUrl: entry.imageUrl }) });
      } catch (error) {
        console.error("[canvas] 创意库图片入库失败，保留原条目:", error);
        items.push(item);
      }
    } else {
      items.push(item);
    }
  }
  return items;
}

async function hydrateKey(key: CanvasPreferenceKey): Promise<void> {
  const stored = await invoke<unknown | null>("canvas_preference_read", { key });
  if (stored !== null && stored !== undefined) {
    cache.set(key, stored);
    forgetLegacy(key);
    publish(key);
    return;
  }
  const legacy = readLegacy(key);
  forgetLegacy(key);
  if (legacy === null) {
    // 明确记录"库里确实还没有"，否则调用方无法区分空数据和未载入。
    cache.set(key, null);
    publish(key);
    return;
  }
  const value = key === "creative-library" ? await upgradeLibraryImages(legacy) : legacy;
  await invoke("canvas_preference_write", { key, value });
  cache.set(key, value);
  publish(key);
}

/** 读缓存值；还没载入时返回 fallback，调用方用 hasCanvasPreference 区分空数据与未载入。 */
export function readCanvasPreference<T>(key: CanvasPreferenceKey, fallback: T): T {
  if (!cache.has(key)) return fallback;
  return cache.get(key) as T;
}

/** 缓存是否已从内核载入，用于区分“没有数据”和“还没读到”。 */
export function hasCanvasPreference(key: CanvasPreferenceKey): boolean {
  return cache.has(key);
}

type PreferenceListener = (key: CanvasPreferenceKey) => void;
const listeners = new Set<PreferenceListener>();

/** 订阅偏好变化（载入完成或本地写入）。 */
export function onCanvasPreferenceChanged(listener: PreferenceListener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function publish(key: CanvasPreferenceKey) {
  listeners.forEach((listener) => {
    try {
      listener(key);
    } catch (error) {
      console.error(`[canvas] 偏好 ${key} 订阅者出错:`, error);
    }
  });
}

/** 覆盖一个偏好文档：先更新缓存，再合并写库。 */
export function writeCanvasPreference(key: CanvasPreferenceKey, value: unknown): void {
  cache.set(key, value);
  publish(key);
  queueFlush(key);
}

function queueFlush(key: CanvasPreferenceKey): void {
  dirty.add(key);
  const pending = timers.get(key);
  if (pending) clearTimeout(pending);
  timers.set(
    key,
    setTimeout(() => {
      timers.delete(key);
      void flushKey(key);
    }, FLUSH_DEBOUNCE_MS),
  );
}

async function flushKey(key: CanvasPreferenceKey): Promise<void> {
  if (!dirty.has(key) || !isTauriRuntime()) return;
  try {
    await invoke("canvas_preference_write", { key, value: cache.get(key) ?? null });
    dirty.delete(key);
  } catch (error) {
    console.error(`[canvas] 保存偏好 ${key} 失败:`, error);
  }
}

/** 载入全部偏好文档（含旧 localStorage 迁移）；重复调用共享同一次结果。 */
export function whenCanvasPreferencesReady(): Promise<void> {
  if (hydration) return hydration;
  hydration = (async () => {
    if (!isTauriRuntime()) {
      CANVAS_PREFERENCE_KEYS.forEach((key) => cache.delete(key));
      return;
    }
    for (const key of CANVAS_PREFERENCE_KEYS) {
      try {
        await hydrateKey(key);
      } catch (error) {
        console.error(`[canvas] 读取偏好 ${key} 失败:`, error);
      }
    }
  })();
  return hydration;
}

/** 退出前把未落库的编辑刷回内核。 */
export async function flushCanvasPreferences(): Promise<void> {
  for (const key of [...dirty]) {
    const pending = timers.get(key);
    if (pending) {
      clearTimeout(pending);
      timers.delete(key);
    }
    await flushKey(key);
  }
}

// 画布窗口一挂载就把偏好拉进内存，调用方仍按同步方式读取。
void whenCanvasPreferencesReady();

// 关窗时先补完防抖窗口内的写入；刷完后 dirty 为空，第二次关闭事件直接放行。
if (isTauriRuntime()) {
  void (async () => {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const appWindow = getCurrentWindow();
    await appWindow.onCloseRequested(async (event) => {
      if (!dirty.size) return;
      event.preventDefault();
      await flushCanvasPreferences();
      await appWindow.close();
    });
  })().catch((error) => console.error("[canvas] 注册关窗刷库失败:", error));
}
