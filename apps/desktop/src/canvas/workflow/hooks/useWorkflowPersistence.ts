import { useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { flushWorkflowPersistence, whenWorkflowPersistenceReady } from "../utils/workflowPersistence";
import { flushCanvasPreferences } from "../utils/canvasPreferences";
import { message } from "../../../components/message";

export function useWorkflowPersistence() {
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let disposed = false;
    setError(null);
    void whenWorkflowPersistenceReady().then(() => {
      if (!disposed) setReady(true);
    }).catch((cause) => {
      console.error("[canvas] 恢复画布失败:", cause);
      if (!disposed) setError(String(cause));
    });
    return () => { disposed = true; };
  }, [attempt]);

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let closing = false;
    let allowClose = false;
    let unlisten: (() => void) | undefined;
    const flush = () => Promise.all([flushWorkflowPersistence(), flushCanvasPreferences()]);

    void import("@tauri-apps/api/window").then(async ({ getCurrentWindow }) => {
      if (disposed) return;
      const appWindow = getCurrentWindow();
      const stop = await appWindow.onCloseRequested(async (event) => {
        if (allowClose) return;
        event.preventDefault();
        if (closing) return;
        closing = true;
        try {
          await flush();
          allowClose = true;
          await appWindow.close();
        } catch (cause) {
          allowClose = false;
          console.error("[canvas] 关闭前保存失败:", cause);
          message.error("画布保存失败，请重试关闭窗口");
        } finally {
          closing = false;
        }
      });
      if (disposed) stop();
      else unlisten = stop;
    }).catch((cause) => console.error("[canvas] 注册关闭前保存失败:", cause));

    return () => {
      disposed = true;
      unlisten?.();
      void flush().catch((cause) => console.error("[canvas] 卸载时保存失败:", cause));
    };
  }, []);

  return { ready, error, retry: () => setAttempt((value) => value + 1) };
}
