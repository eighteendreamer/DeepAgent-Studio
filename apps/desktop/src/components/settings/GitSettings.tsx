import { useEffect, useState } from "react";
import {
  DEFAULT_GIT_UI_SETTINGS,
  getGitUiSettings,
  saveGitUiSettings,
  type GitUiSettings,
} from "../git/gitSettings";

function ToggleSwitch({ checked, onChange }: { checked: boolean; onChange: () => void }) {
  return (
    <button
      type="button"
      aria-pressed={checked}
      className={`relative h-5 w-9 rounded-full transition-colors ${checked ? "bg-blue-500" : "bg-gray-300"}`}
      onClick={onChange}
    >
      <span
        className={`absolute left-[3px] top-[3px] h-3.5 w-3.5 rounded-full bg-white transition-transform ${
          checked ? "translate-x-4" : "translate-x-0"
        }`}
      />
    </button>
  );
}

export function GitSettings() {
  const [settings, setSettings] = useState<GitUiSettings>(DEFAULT_GIT_UI_SETTINGS);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    setSettings(getGitUiSettings());
  }, []);

  const update = (patch: Partial<GitUiSettings>) => {
    setSaved(false);
    setSettings((current) => ({ ...current, ...patch }));
  };

  const save = () => {
    setSettings(saveGitUiSettings(settings));
    setSaved(true);
  };

  return (
    <>
      <div className="mb-10 max-w-[760px]">
        <h1 className="mb-1 text-2xl font-semibold text-text-base">Git</h1>
        <div className="text-[13px] text-text-secondary">
          控制当前项目的分支显示、提交和上传行为。
        </div>
      </div>

      <div className="mb-10 max-w-[760px] overflow-hidden rounded-xl border border-border-theme bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
        <div className="flex items-center justify-between border-b border-border-theme p-4">
          <div className="pr-4">
            <div className="mb-1 text-[14px] font-medium text-text-base">分支前缀</div>
            <div className="text-[12px] text-text-secondary">创建新分支时默认使用的前缀。</div>
          </div>
          <input
            type="text"
            value={settings.branchPrefix}
            onChange={(event) => update({ branchPrefix: event.target.value })}
            className="w-[220px] rounded-lg border border-border-theme bg-white px-3 py-1.5 text-[13px] shadow-sm focus:border-blue-500 focus:outline-none"
            placeholder="codex/"
          />
        </div>

        <div className="flex items-center justify-between border-b border-border-theme p-4">
          <div className="pr-4">
            <div className="mb-1 text-[14px] font-medium text-text-base">上传前确认</div>
            <div className="text-[12px] text-text-secondary">
              单分支上传和多项目同分支上传前弹出确认。
            </div>
          </div>
          <ToggleSwitch
            checked={settings.confirmBeforePush}
            onChange={() => update({ confirmBeforePush: !settings.confirmBeforePush })}
          />
        </div>

        <div className="flex items-center justify-between border-b border-border-theme p-4">
          <div className="pr-4">
            <div className="mb-1 text-[14px] font-medium text-text-base">批量提交自动暂存</div>
            <div className="text-[12px] text-text-secondary">
              批量提交时默认暂存每个项目的全部 Git 变更。
            </div>
          </div>
          <ToggleSwitch
            checked={settings.batchStageAll}
            onChange={() => update({ batchStageAll: !settings.batchStageAll })}
          />
        </div>

        <div className="p-4">
          <div className="mb-2">
            <div className="mb-1 text-[14px] font-medium text-text-base">提交说明</div>
            <div className="text-[12px] text-text-secondary">保存本机提交信息偏好，供后续生成提交说明时使用。</div>
          </div>
          <textarea
            value={settings.commitInstructions}
            onChange={(event) => update({ commitInstructions: event.target.value })}
            className="h-[120px] w-full resize-none rounded-lg border border-border-theme bg-white px-3 py-2 text-[13px] text-text-base focus:border-blue-500 focus:outline-none"
            placeholder="例如：提交信息使用中文，首行不超过 50 个字。"
          />
        </div>
      </div>

      <div className="sticky bottom-0 flex max-w-[760px] justify-end bg-bg-primary/80 py-4 backdrop-blur">
        {saved && <span className="mr-3 self-center text-[12px] text-green-600">已保存</span>}
        <button
          type="button"
          onClick={save}
          className="rounded-full bg-blue-500 px-5 py-2 text-[13px] font-medium text-white shadow-sm transition-colors hover:bg-blue-600"
        >
          保存
        </button>
      </div>
    </>
  );
}
