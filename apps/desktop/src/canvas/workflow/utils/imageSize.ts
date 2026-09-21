/**
 * 图片节点的输出尺寸。
 *
 * 内核的图片执行器只认 config 里的 `size`（"WxH" 或 "auto"），网关原样转给供应商。
 * 面板上的比例 / 分辨率只是这个值的一对编辑器，折算规则集中在这里，
 * 免得第二处再算一遍算出不一样的结果。
 */

/** 分辨率档位 → 长边像素。 */
const RESOLUTION_LONG_EDGE: Record<string, number> = {
  "1K": 1024,
  "2K": 2048,
  "4K": 4096,
};

const DEFAULT_RESOLUTION = "1K";
/** 宽高向 16 对齐并留 64 下限，是图像编码与多数供应商的通用约束。 */
const ALIGNMENT = 16;
const MIN_EDGE = 64;

function alignEdge(value: number): number {
  return Math.max(MIN_EDGE, Math.round(value / ALIGNMENT) * ALIGNMENT);
}

/** 解析 "1024x1024" / "1024×1024" 形式的自定义尺寸。 */
export function parseCustomSize(value: string | undefined): { w: number; h: number } | null {
  if (!value) return null;
  const matched = value.match(/^(\d+)\s*[xX×]\s*(\d+)$/);
  if (!matched) return null;
  const w = Number(matched[1]);
  const h = Number(matched[2]);
  if (!w || !h) return null;
  return { w, h };
}

/**
 * 比例 + 分辨率 → 内核认识的 size 字符串。
 *
 * Auto 与无法解析的输入都回 "auto"，把决定权交给供应商，而不是前端猜一个像素值。
 */
export function imageSizeFor(ratio: string, resolution: string, customSize?: string): string {
  if (ratio === "custom") {
    const parsed = parseCustomSize(customSize);
    return parsed ? `${parsed.w}x${parsed.h}` : "auto";
  }
  if (!ratio || ratio === "Auto") return "auto";
  const parts = ratio.split(":").map(Number);
  if (parts.length !== 2 || !parts[0] || !parts[1]) return "auto";
  const [ratioW, ratioH] = parts;
  const longEdge = RESOLUTION_LONG_EDGE[resolution] ?? RESOLUTION_LONG_EDGE[DEFAULT_RESOLUTION];
  if (ratioW >= ratioH) {
    return `${longEdge}x${alignEdge((longEdge * ratioH) / ratioW)}`;
  }
  return `${alignEdge((longEdge * ratioW) / ratioH)}x${longEdge}`;
}
