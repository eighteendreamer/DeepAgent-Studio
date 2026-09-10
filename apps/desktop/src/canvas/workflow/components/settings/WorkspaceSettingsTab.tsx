import { FolderOpen } from "lucide-react";
import { Button } from "../../../../components/shadcn/button";
import { Input } from "../../../../components/shadcn/input";
import { Label } from "../../../../components/shadcn/label";
import { useCanvasSettingsStore } from "../../store/canvasSettingsStore";
import { isTauri } from "../../../../api";

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
  const tauri = isTauri();

  const handleBrowse = async (kind: "imageDir" | "videoDir") => {
    const dir = await pickDirectory();
    if (dir) setWorkspaceDir(kind, dir);
  };

  return (
    <>
      <h2 className="text-lg font-semibold mb-1" style={{ color: "var(--theme-fg, #111)" }}>
        工作区设置
      </h2>
      <p className="text-[12px] mb-6" style={{ color: "var(--theme-text-secondary, #666)" }}>
        指定图片与视频生成结果的本地缓存目录。
      </p>

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
