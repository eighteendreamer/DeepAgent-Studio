import {
  hasCanvasPreference,
  onCanvasPreferenceChanged,
  readCanvasPreference,
  writeCanvasPreference,
} from "./canvasPreferences";

/**
 * 创意库文档的内核读写入口。
 *
 * 面板和节点工具栏都会改动同一份库，这里作为唯一入口，避免两处各自
 * 解析存储格式。
 */

export interface CreativeItem {
  id: string;
  name: string;
  category: string;
  prompt?: string;
  imageUrl?: string;
  isFavorite?: boolean;
  createdAt: number | string;
  order?: number;
}

/** 偏好尚未从内核载入时返回 null，调用方据此区分空库和未读到。 */
export function readCreativeLibrary(): CreativeItem[] | null {
  if (!hasCanvasPreference("creative-library")) return null;
  const value = readCanvasPreference<unknown>("creative-library", null);
  return Array.isArray(value) ? (value as CreativeItem[]) : [];
}

export function writeCreativeLibrary(items: CreativeItem[]): void {
  writeCanvasPreference("creative-library", items);
}

/** 追加一条素材（新条目在前）；库还没载入就失败，不用空列表覆盖真实数据。 */
export function prependCreativeItem(item: CreativeItem): void {
  const items = readCreativeLibrary();
  if (items === null) {
    throw new Error("创意库正在从内核数据库载入，请稍后重试。");
  }
  writeCreativeLibrary([item, ...items]);
}

export function onCreativeLibraryChanged(listener: () => void): () => void {
  return onCanvasPreferenceChanged((key) => {
    if (key === "creative-library") listener();
  });
}
