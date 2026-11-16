/** Do not expose markdown source while an async renderer is being prepared. */
export function MarkdownLoading() {
  return <div role="status" aria-label="正在渲染消息" aria-busy="true" className="space-y-2 py-2">
    <div aria-hidden="true" className="h-3 w-3/4 rounded bg-ui-tint motion-safe:animate-pulse" />
    <div aria-hidden="true" className="h-3 w-1/2 rounded bg-ui-tint motion-safe:animate-pulse" />
  </div>;
}
