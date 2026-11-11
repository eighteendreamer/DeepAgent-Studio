/** Early UI feedback only. The backend validates the original raster bytes. */
export function validateComposerImage(file: Pick<File, "type" | "size">): void {
  const mime = file.type.toLowerCase();
  if (!["image/jpeg", "image/png", "image/gif", "image/webp"].includes(mime)) {
    throw new Error("图片仅支持 JPEG、PNG、GIF 和 WebP，请选择受支持的图片。");
  }
  if (file.size <= 0 || file.size > 32 * 1024 * 1024) {
    throw new Error("图片不能为空，且不能超过 32 MiB。");
  }
}
