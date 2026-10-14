import type { ComposerFileChange, ComposerTodo } from "./ComposerActivityBar";

// Fixed design data for this UI iteration; replace at the ChatView boundary
// with the existing todo_write snapshot and useGitStatus projection later.
export const composerActivityMock: { todos: ComposerTodo[]; changes: ComposerFileChange[] } = {
  todos: [
    { id: "1", text: "梳理任务目标和现有实现", status: "completed" },
    { id: "2", text: "确认待办与文件变更的展示位置", status: "completed" },
    { id: "3", text: "调整输入框上方的摘要条与列表样式", status: "in_progress" },
    { id: "4", text: "整理列表中的任务状态与进度", status: "pending" },
    { id: "5", text: "展示变更文件及新增、删除行数", status: "pending" },
    { id: "6", text: "检查长文件路径和窄窗口布局", status: "pending" },
    { id: "7", text: "验证列表切换、关闭与键盘操作", status: "pending" },
    { id: "8", text: "运行前端构建并复核界面", status: "pending" },
    { id: "9", text: "整理结果并提交本轮修改", status: "pending" },
  ],
  changes: [
    { path: "apps/desktop/src/components/ChatView.tsx", additions: 128, deletions: 76 },
    { path: "apps/desktop/src/components/ComposerActivityBar.tsx", additions: 88, deletions: 32 },
    { path: "apps/desktop/src/styles.css", additions: 16, deletions: 14 },
  ],
};
