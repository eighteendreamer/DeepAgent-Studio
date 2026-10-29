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
 * Legacy `localStorage` payloads are upgraded (inline base64 images become
 * artifacts) and removed only after this window's database write succeeds.
 * Ambiguous leftovers are retained, not treated as a second writable store.
 */

export type CanvasPreferenceKey =
  | "creative-library"
  | "workflow-snippets"
  | "workspace-dirs"
  | "video-edit-projects";

export const CANVAS_PREFERENCE_KEYS: CanvasPreferenceKey[] = [
  "creative-library",
  "workflow-snippets",
  "workspace-dirs",
  "video-edit-projects",
];

/** Keys written by the pre-database canvas. */
const LEGACY_STORAGE_KEYS: Partial<Record<CanvasPreferenceKey, string>> = {
  "creative-library": "canvas-creative-library",
  "workflow-snippets": "workflow-canvas-professional-snippets",
  "workspace-dirs": "workflow-workspace",
};

/** 连续编辑合并落库。 */
const FLUSH_DEBOUNCE_MS = 400;

const cache = new Map<CanvasPreferenceKey, unknown>();
const dirty = new Set<CanvasPreferenceKey>();
const timers = new Map<CanvasPreferenceKey, ReturnType<typeof setTimeout>>();
const revisions = new Map<CanvasPreferenceKey, number>();
const flushes = new Map<CanvasPreferenceKey, Promise<void>>();
// Prepared migration values are not readable/editable cache entries. Retain
// them on write failure so retrying the same source does not re-import media.
const migrations = new Map<CanvasPreferenceKey, { raw: string; value: unknown }>();
const persistenceWaiters = new Map<CanvasPreferenceKey, Array<{
  resolve: () => void;
  reject: (error: Error) => void;
}>>();

let hydration: Promise<void> | null = null;

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function readLegacy(key: CanvasPreferenceKey): { raw: string; value: unknown } | null {
  const legacyKey = LEGACY_STORAGE_KEYS[key];
  if (!legacyKey) return null;
  let raw: string | null;
  try {
    raw = localStorage.getItem(legacyKey);
  } catch {
    throw new Error(`无法读取旧画布偏好 ${key}，源数据未修改`);
  }
  if (raw === null) return null;
  try {
    return { raw, value: JSON.parse(raw) };
  } catch {
    // JSON.parse messages can contain fragments of user data; do not log them.
    throw new Error(`旧画布偏好 ${key} JSON 无效，源数据已保留`);
  }
}

function forgetLegacy(key: CanvasPreferenceKey, raw: string) {
  try {
    const legacyKey = LEGACY_STORAGE_KEYS[key];
    if (!legacyKey) return;
    if (localStorage.getItem(legacyKey) === raw) localStorage.removeItem(legacyKey);
    else console.warn(`[canvas] 旧偏好 ${key} 已变化，保留源数据`);
  } catch {
    // The database commit is acknowledged; a cleanup failure must not make
    // the next startup overwrite it by migrating again.
    console.warn(`[canvas] 旧偏好 ${key} 清理失败；数据库已保存，保留源数据`);
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
    migrations.delete(key);
    cache.set(key, stored);
    // Without a receipt tying legacy bytes to this document, do not erase a
    // possibly different source left by an older window or an unknown commit.
    publish(key);
    return;
  }
  const legacy = readLegacy(key);
  if (legacy === null) {
    migrations.delete(key);
    // 明确记录"库里确实还没有"，否则调用方无法区分空数据和未载入。
    cache.set(key, null);
    publish(key);
    return;
  }
  let migration = migrations.get(key);
  if (!migration || migration.raw !== legacy.raw) {
    const value = key === "creative-library"
      ? await upgradeLibraryImages(legacy.value)
      : legacy.value;
    migration = { raw: legacy.raw, value };
    migrations.set(key, migration);
  }
  await invoke("canvas_preference_write", { key, value: migration.value });
  forgetLegacy(key, migration.raw);
  migrations.delete(key);
  cache.set(key, migration.value);
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

/** 只有成功读取（含明确空值）的文档才能被替换，fallback 不是写入基线。 */
export function writeCanvasPreference(key: CanvasPreferenceKey, value: unknown): void {
  if (isTauriRuntime() && !cache.has(key)) {
    throw new Error(`画布偏好 ${key} 尚未成功读取，请重新加载`);
  }
  cache.set(key, value);
  revisions.set(key, (revisions.get(key) ?? 0) + 1);
  publish(key);
  queueFlush(key);
}

/** Write through the shared debounce and resolve only when the latest value is durable. */
export async function writeCanvasPreferenceAndWait(
  key: CanvasPreferenceKey,
  value: unknown,
): Promise<void> {
  writeCanvasPreference(key, value);
  if (!isTauriRuntime()) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const waiters = persistenceWaiters.get(key) ?? [];
    waiters.push({ resolve, reject });
    persistenceWaiters.set(key, waiters);
  });
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
  const existing = flushes.get(key);
  if (existing) return existing;
  const pending = (async () => {
    while (dirty.has(key)) {
      const revision = revisions.get(key) ?? 0;
      try {
        await invoke("canvas_preference_write", { key, value: cache.get(key) ?? null });
      } catch (cause) {
        const error = cause instanceof Error ? cause : new Error(String(cause));
        console.error(`[canvas] 保存偏好 ${key} 失败:`, error);
        const waiters = persistenceWaiters.get(key) ?? [];
        persistenceWaiters.delete(key);
        waiters.forEach(({ reject }) => reject(error));
        return;
      }
      if (revision !== (revisions.get(key) ?? 0)) continue;
      dirty.delete(key);
      const waiters = persistenceWaiters.get(key) ?? [];
      persistenceWaiters.delete(key);
      waiters.forEach(({ resolve }) => resolve());
    }
  })().finally(() => flushes.delete(key));
  flushes.set(key, pending);
  return pending;
}

/** 共享正在执行的载入；失败可重试，但不能重读已载入文档并覆盖本地编辑。 */
export function whenCanvasPreferencesReady(): Promise<void> {
  if (hydration) return hydration;
  hydration = (async () => {
    if (!isTauriRuntime()) {
      CANVAS_PREFERENCE_KEYS.forEach((key) => cache.delete(key));
      return;
    }
    const failures: string[] = [];
    for (const key of CANVAS_PREFERENCE_KEYS) {
      if (cache.has(key)) continue;
      try {
        await hydrateKey(key);
      } catch (error) {
        failures.push(`${key}: ${String(error)}`);
      }
    }
    if (failures.length) throw new Error(`读取画布偏好失败：${failures.join("；")}`);
  })().catch((error) => {
    hydration = null;
    throw error;
  });
  return hydration;
}

/** 退出前把未落库的编辑刷回内核。 */
export async function flushCanvasPreferences(): Promise<void> {
  await hydration;
  if (isTauriRuntime() && CANVAS_PREFERENCE_KEYS.some((key) => !cache.has(key))) {
    throw new Error("画布偏好尚未成功读取，请重新加载后保存");
  }
  for (const key of [...dirty]) {
    const pending = timers.get(key);
    if (pending) {
      clearTimeout(pending);
      timers.delete(key);
    }
    await flushKey(key);
  }
  if (dirty.size) throw new Error("画布偏好尚未保存");
}

// 画布窗口一挂载就把偏好拉进内存，调用方仍按同步方式读取。
void whenCanvasPreferencesReady().catch((error) => {
  console.error("[canvas] 初始化偏好失败:", error);
});

// 关闭前由 useWorkflowPersistence 统一等待偏好与节点图，避免两个监听器分别重发关闭。
