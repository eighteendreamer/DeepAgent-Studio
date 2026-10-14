/// <reference types="vite/client" />
// Actual ChatView with fixed conversation data. No model or Git writes.
import React from "react";
import { createRoot } from "react-dom/client";
import "../src/i18n";
import "../src/icons";
import "../src/styles.css";
import { ChatView } from "../src/components/ChatView";

const previewRoot = createRoot(document.getElementById("root")!);
import.meta.hot?.dispose(() => previewRoot.unmount());
previewRoot.render(
  <React.StrictMode>
    <div style={{ height: "100dvh", display: "flex", flexDirection: "column" }}>
      <ChatView
        title="待办清单与文件变更"
        messages={[
          { role: "user", content: "把任务进度和文件变更放在输入框上方。" },
          { role: "assistant", content: "已整理好待办清单和文件变更。\n\n输入框上方显示当前进度，点击待办可查看步骤，点击文件变更可查看各文件的新增和删除行数。" },
        ]}
        onSend={() => {}}
      />
    </div>
  </React.StrictMode>,
);
