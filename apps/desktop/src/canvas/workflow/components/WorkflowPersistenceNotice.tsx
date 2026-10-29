import { Button } from "../../../components/shadcn/button";
import { Item, ItemActions, ItemContent, ItemDescription, ItemTitle } from "../../../components/shadcn/item";
import type { CanvasMode, WorkflowPersistenceProblem } from "../utils/workflowPersistence";

interface Props {
  problems: WorkflowPersistenceProblem[];
  preserveAndReload: (mode: CanvasMode) => Promise<void>;
  retrySave: (mode: CanvasMode) => Promise<void>;
}

export function WorkflowPersistenceNotice({ problems, preserveAndReload, retrySave }: Props) {
  if (!problems.length) return null;
  return (
    <div className="absolute left-1/2 top-16 z-[100] flex w-[min(680px,calc(100%-32px))] -translate-x-1/2 flex-col gap-2">
      {problems.map((problem) => (
        <Item key={problem.mode} role="alert" variant="outline" size="sm" className="flex-wrap border-amber-400/50 bg-elevated-bg text-text-base shadow-xl">
          <ItemContent>
            <ItemTitle>{problem.mode === "creative" ? "创意" : "专业"}画布{problem.conflict ? "版本冲突" : "尚未保存"}</ItemTitle>
            <ItemDescription>{problem.message}</ItemDescription>
            {problem.conflict && <ItemDescription>本地画布保持不变，自动保存已暂停。操作会先将本地图保存到工作流库，再载入最新版本。</ItemDescription>}
          </ItemContent>
          <ItemActions>
            <Button variant="secondary" size="sm" disabled={problem.recovering} onClick={() => {
              const action = problem.conflict ? preserveAndReload : retrySave;
              void action(problem.mode).catch((cause) => console.error("[canvas] 保存恢复失败:", cause));
            }}>
              {problem.recovering ? "正在保留副本…" : problem.conflict ? "保留副本并载入最新" : "重试保存"}
            </Button>
          </ItemActions>
        </Item>
      ))}
    </div>
  );
}
