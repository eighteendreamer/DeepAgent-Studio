/** Prefix offsets include the trailing edge, so empty lists still have offset 0. */
export function virtualListOffsets(
  keys: readonly string[],
  heights: ReadonlyMap<string, number>,
  estimatedHeight: number,
): number[] {
  const offsets = [0];
  for (const key of keys) offsets.push(offsets[offsets.length - 1] + (heights.get(key) ?? estimatedHeight));
  return offsets;
}

export function virtualListRange(offsets: readonly number[], top: number, height: number, overscan: number) {
  const count = offsets.length - 1;
  const scrollTop = Math.max(0, Math.min(top, Math.max(0, offsets[count] - height)));
  const lowerBound = (value: number) => {
    let low = 0, high = offsets.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (offsets[middle] < value) low = middle + 1;
      else high = middle;
    }
    return low;
  };
  return {
    scrollTop,
    start: Math.max(0, lowerBound(Math.max(0, scrollTop - overscan)) - 1),
    end: Math.min(count, lowerBound(scrollTop + height + overscan)),
  };
}
