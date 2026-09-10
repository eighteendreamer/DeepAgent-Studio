import { FolderOpen, Download, Upload } from "lucide-react";
import { Button } from "../../../../components/shadcn/button";
import { Input } from "../../../../components/shadcn/input";
import { Label } from "../../../../components/shadcn/label";
import { useCanvasSettingsStore } from "../../store/canvasSettingsStore";
import { useCanvasStore } from "../../store/canvasStore";
import { useCreativeStore } from "../../store/creativeStore";
import { useProfessionalStore } from "../../store/professionalStore";
import { isTauri } from "../../../../api";
import {
  exportWorkflow,
  downloadWorkflow,
  parseWorkflowImport,
} from "../../utils/workflowImportExport";
import { useRef } from "react";

async function pickDirectory(): Promise<string | null> {
  if (!isTauri()) return null;
  const mod = await import("@tauri-apps/plugin-dialog");
  const selected = await mod.open({ directory: true, multiple: false, title: "选择缓存目录" });
  if (typeof selected === "string") return selected;
  return null;
}

interface DirRowProps {
  label: string;
  desc: string;
  value: string;
  onBrowse: () => void;
  onClear: () => void;
  disabled: boolean;
  disabledHint: string;
}

function DirRow({ label, desc, value, onBrowse, onClear, disabled, disabledHint }: DirRowProps) {
  return (
    <div
      className="rounded-lg border px-3 py-3 space-y-2"
      style={{ borderColor: "var(--theme-border, #ddd)" }}
    >
      <div>
        <Label className="text-[13px] font-medium" style={{ color: "var(--theme-fg, #111)" }}>
          {label}
        </Label>
        <div className="text-[11px]" style={{ color: "var(--theme-text-secondary, #999)" }}>
          {desc}
        </div>
      </div>
      <div className="flex items-center gap-2">
        <Input
          readOnly
          value={value}
          placeholder={disabled ? disabledHint : "未设置"}
          className="h-8 flex-1 text-[12px]"
        />
        <Button
          variant="outline"
          size="sm"
          className="h-8 text-[12px]"
          disabled={disabled}
          title={disabled ? disabledHint : "浏览"}
          onClick={onBrowse}
        >
          <FolderOpen size={13} className="mr-1" />
          浏览
        </Button>
        {value && (
          <Button variant="ghost" size="sm" className="h-8 text-[12px]" onClick={onClear}>
            清除
          </Button>
        )}
      </div>
    </div>
  );
}

export function WorkspaceSettingsTab() {
  const workspace = useCanvasSettingsStore((s) => s.workspace);
  const setWorkspaceDir = useCanvasSettingsStore((s) => s.setWorkspaceDir);
  const mode = useCanvasStore((s) => s.mode);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const tauri = isTauri();

  const handleBrowse = async (kind: "imageDir" | "videoDir") => {
    const dir = await pickDirectory();
    if (dir) setWorkspaceDir(kind, dir);
  };

  const handleExport = () => {
    const store = mode === "creative" ? useCreativeStore.getState() : useProfessionalStore.getState();
    const data = exportWorkflow(mode, store.nodes, store.edges);
    downloadWorkflow(data, `workflow-${mode}-${Date.now()}.json`);
  };

  const handleImportClick = () => {
    fileInputRef.current?.click();
  };

  const handleImportFile = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;

    const text = await file.text();
    const result = parseWorkflowImport(text);

    if (!result.ok || !result.data) {
      window.alert(`导入失败：${result.error}`);
      return;
    }

    if (result.data.mode !== mode) {
      const confirmed = window.confirm(
        `该工作流属于${result.data.mode === "creative" ? "创作" : "专业"}模式，当前为${mode === "creative" ? "创作" : "专业"}模式。是否切换模式并导入？`,
      );
      if (!confirmed) return;
      useCanvasStore.getState().setMode(result.data.mode);
    }

    const store = result.data.mode === "creative" ? useCreativeStore.getState() : useProfessionalStore.getState();
    store.pushHistory();
    store.setNodes(result.data.nodes);
    store.setEdges(result.data.edges);

    window.alert("导入成功");
    e.target.value = "";
  };

  return (
    <>
      <h2 className="text-lg font-semibold mb-1" style={{ color: "var(--theme-fg, #111)" }}>
        工作区设置
      </h2>
      <p className="text-[12px] mb-6" style={{ color: "var(--theme-text-secondary, #666)" }}>
        指定图片与视频生成结果的本地缓存目录，或导入/导出工作流。
      </p>

      {/* Import/Export */}
      <div className="mb-6 p-4 rounded-lg border" style={{ borderColor: "var(--theme-border, #ddd)" }}>
        <Label className="text-[13px] font-medium mb-3 block" style={{ color: "var(--theme-fg, #111)" }}>
          工作流导入导出
        </Label>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="sm" className="h-8 text-[12px]" onClick={handleExport}>
            <Download size={13} className="mr-1" />
            导出
          </Button>
          <Button variant="outline" size="sm" className="h-8 text-[12px]" onClick={handleImportClick}>
            <Upload size={13} className="mr-1" />
            导入
          </Button>
          <input
            ref={fileInputRef}
            type="file"
            accept=".json"
            className="hidden"
            onChange={handleImportFile}
          />
        </div>
        <div className="text-[11px] mt-2" style={{ color: "var(--theme-text-secondary, #999)" }}>
          导出当前画布为 JSON 文件，或从 JSON 文件导入工作流
        </div>
      </div>

      <div className="space-y-4">
        <DirRow
          label="图片输出目录"
          desc="生图节点生成的图片保存到此目录"
          value={workspace.imageDir}
          onBrowse={() => handleBrowse("imageDir")}
          onClear={() => setWorkspaceDir("imageDir", "")}
          disabled={!tauri}
          disabledHint="仅桌面应用可用"
        />
        <DirRow
          label="视频输出目录"
          desc="视频节点生成的视频保存到此目录"
          value={workspace.videoDir}
          onBrowse={() => handleBrowse("videoDir")}
          onClear={() => setWorkspaceDir("videoDir", "")}
          disabled={!tauri}
          disabledHint="仅桌面应用可用"
        />
      </div>
    </>
  );
}
