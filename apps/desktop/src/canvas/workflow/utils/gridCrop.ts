export interface GridCropRegion {
  index: number;
  row: number;
  col: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface GridCropFileResult {
  region: GridCropRegion;
  dataUrl: string;
}

const SUPPORTED_GRIDS = new Set([4, 9, 16, 25]);

export function getGridSide(gridCount: number): number {
  if (!SUPPORTED_GRIDS.has(gridCount)) throw new Error(`Unsupported grid count: ${gridCount}`);
  return Math.round(Math.sqrt(gridCount));
}

export function buildGridRegions(sourceWidth: number, sourceHeight: number, gridCount: number): GridCropRegion[] {
  const side = getGridSide(gridCount);
  const cellW = Math.floor(sourceWidth / side);
  const cellH = Math.floor(sourceHeight / side);
  const regions: GridCropRegion[] = [];

  for (let row = 0; row < side; row++) {
    for (let col = 0; col < side; col++) {
      const isLastCol = col === side - 1;
      const isLastRow = row === side - 1;
      regions.push({
        index: row * side + col + 1,
        row,
        col,
        x: col * cellW,
        y: row * cellH,
        width: isLastCol ? sourceWidth - col * cellW : cellW,
        height: isLastRow ? sourceHeight - row * cellH : cellH,
      });
    }
  }
  return regions;
}

function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = url;
  });
}

function canvasToDataUrl(canvas: HTMLCanvasElement): string {
  return canvas.toDataURL("image/png");
}

export async function cropImageToGrid(imageUrl: string, gridCount: number): Promise<GridCropFileResult[]> {
  const img = await loadImage(imageUrl);
  const regions = buildGridRegions(img.naturalWidth, img.naturalHeight, gridCount);
  const results: GridCropFileResult[] = [];

  for (const region of regions) {
    const canvas = document.createElement("canvas");
    canvas.width = region.width;
    canvas.height = region.height;
    const ctx = canvas.getContext("2d");
    if (!ctx) continue;
    ctx.drawImage(img, region.x, region.y, region.width, region.height, 0, 0, region.width, region.height);
    results.push({ region, dataUrl: canvasToDataUrl(canvas) });
  }
  return results;
}

export async function cropImageToRect(imageUrl: string, sx: number, sy: number, sw: number, sh: number): Promise<string> {
  const img = await loadImage(imageUrl);
  const canvas = document.createElement("canvas");
  canvas.width = sw;
  canvas.height = sh;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("Cannot get canvas context");
  ctx.drawImage(img, sx, sy, sw, sh, 0, 0, sw, sh);
  return canvasToDataUrl(canvas);
}
