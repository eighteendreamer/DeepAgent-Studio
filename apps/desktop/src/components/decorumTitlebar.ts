export const NAV_SLOT_ID = "decorum-nav-slot";
export const NAV_ROW_ID = "decorum-nav-row";

/** decorum 只为 main 窗口创建 overlay 标题栏；session-* 子窗口保留系统标题栏，不渲染。 */
export async function isMainWindow(): Promise<boolean> {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return true;
  }
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    return getCurrentWindow().label === "main";
  } catch {
    return true;
  }
}

/** 插件在页面加载后才注入 DOM，且目标可能嵌在容器内，等它出现再挂内容。 */
export function waitForDecorumElement(
  selector: string,
  timeoutMs = 5000,
): Promise<HTMLElement> {
  return new Promise((resolve, reject) => {
    const existing = document.querySelector<HTMLElement>(selector);
    if (existing) {
      resolve(existing);
      return;
    }
    const observer = new MutationObserver(() => {
      const el = document.querySelector<HTMLElement>(selector);
      if (el) {
        observer.disconnect();
        resolve(el);
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
    setTimeout(() => {
      observer.disconnect();
      reject(new Error(`decorum element not found: ${selector}`));
    }, timeoutMs);
  });
}
