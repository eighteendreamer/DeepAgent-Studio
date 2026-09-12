import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import {
  ChevronDown,
  Image as ImageIcon,
  Maximize2,
  MessageCircle,
  Send,
  Sparkles,
  X,
} from "lucide-react";
import { Button } from "../../../components/shadcn/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "../../../components/shadcn/dropdown-menu";
import { Textarea } from "../../../components/shadcn/textarea";

interface Props {
  imageUrl: string;
  itemName: string;
  onCancel: () => void;
}

type Message = {
  id: number;
  content: string;
};

function SettingMenu({ label, value, options, onChange }: {
  label: string;
  value: string;
  options: string[];
  onChange: (value: string) => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-left transition-colors hover:bg-white/[0.08]"
        >
          <span className="text-[11px] text-white/40">{label}</span>
          <span className="max-w-[92px] truncate text-[12px] font-medium text-white/82">{value}</span>
          <ChevronDown className="h-3.5 w-3.5 shrink-0 text-white/35" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="!z-[11000] min-w-[150px] !bg-[rgba(28,28,34,0.96)] !text-white">
        {options.map((option) => (
          <DropdownMenuItem
            key={option}
            onClick={() => onChange(option)}
            className="!text-[12px] !text-white/85 data-[highlighted]:!bg-white/10"
          >
            {option}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function OutpaintOverlay({ imageUrl, itemName, onCancel }: Props) {
  const [draft, setDraft] = useState("");
  const [model, setModel] = useState("GPT Image 2");
  const [resolution, setResolution] = useState("4K");
  const [thinkingLevel, setThinkingLevel] = useState("中度");
  const [preset, setPreset] = useState("预设");
  const [messages, setMessages] = useState<Message[]>([]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onCancel]);

  const sendMessage = () => {
    const content = draft.trim() || "使用默认提示词，自然延展画面";
    setMessages((current) => [...current, { id: Date.now(), content }]);
    setDraft("");
  };

  return createPortal(
    <div
      className="fixed inset-0 z-[10000] overflow-hidden bg-black/78 backdrop-blur-md"
      onMouseDown={(event) => event.stopPropagation()}
      onDoubleClick={(event) => event.stopPropagation()}
      onContextMenu={(event) => event.stopPropagation()}
    >
      <div className="absolute inset-0" onClick={onCancel} />

      <header className="absolute left-6 right-6 top-5 z-10 flex items-center justify-between">
        <div className="flex min-w-0 items-center gap-3 rounded-2xl border border-white/10 bg-black/35 px-3 py-2 backdrop-blur-xl">
          <div className="flex h-8 w-8 items-center justify-center rounded-xl bg-white/[0.08] text-white/75">
            <Maximize2 className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h1 className="text-[14px] font-semibold text-white/90">智能扩图</h1>
              <span className="text-[11px] text-white/35">·</span>
              <span className="max-w-[260px] truncate text-[11px] text-white/45">{itemName}</span>
            </div>
            <p className="text-[10px] text-white/30">图片预览 · 扩图编辑</p>
          </div>
        </div>
        <button
          type="button"
          onClick={onCancel}
          className="rounded-xl border border-white/10 bg-black/35 p-2.5 text-white/60 backdrop-blur-xl transition-colors hover:bg-white/10 hover:text-white"
          title="关闭"
        >
          <X className="h-5 w-5" />
        </button>
      </header>

      <main className="absolute inset-0 flex items-center justify-center px-8 pb-[238px] pt-24">
        <div className="relative inline-flex max-h-full max-w-full overflow-hidden rounded-[22px] border border-white/10 bg-black/25 shadow-[0_20px_80px_rgba(0,0,0,0.32)]">
          <img
            src={imageUrl}
            alt={itemName}
            className="block max-h-[calc(100vh-300px)] max-w-[calc(100vw-64px)] rounded-[21px] object-contain"
            draggable={false}
          />
          <div className="absolute bottom-3 left-4 flex items-center gap-1.5 text-[10px] text-white/35">
            <ImageIcon className="h-3.5 w-3.5" />
            原图预览
          </div>
        </div>
      </main>

      <section className="absolute bottom-5 left-1/2 z-10 w-[min(560px,calc(100vw-48px))] -translate-x-1/2 rounded-2xl border border-white/10 bg-[rgba(18,18,22,0.94)] p-3 shadow-[0_16px_50px_rgba(0,0,0,0.45)] backdrop-blur-2xl">
        {messages.length > 0 && (
          <div className="mb-2 flex max-h-9 items-center gap-2 overflow-x-auto px-1 text-[11px] text-white/50">
            <MessageCircle className="h-3.5 w-3.5 shrink-0 text-blue-400" />
            {messages[messages.length - 1].content}
          </div>
        )}
        <div className="flex items-start gap-2">
          <Sparkles className="mt-2 h-4 w-4 shrink-0 text-white/45" />
          <Textarea
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
                event.preventDefault();
                sendMessage();
              }
            }}
            placeholder="描述扩展区域的内容（留空使用默认提示词）"
            className="min-h-[58px] flex-1 resize-none border-0 bg-transparent px-0 py-1.5 !text-[12px] !text-white/85 shadow-none placeholder:!text-white/30 focus-visible:!ring-0"
          />
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-1 border-t border-white/[0.08] pt-2">
          <SettingMenu label="模型" value={model} options={["GPT Image 2", "DALL-E 3"]} onChange={setModel} />
          <SettingMenu label="分辨率" value={resolution} options={["1K", "2K", "4K"]} onChange={setResolution} />
          <SettingMenu label="思考等级" value={thinkingLevel} options={["简单", "中度", "深度"]} onChange={setThinkingLevel} />
          <SettingMenu
            label="预设"
            value={preset}
            options={["1.5x", "2x", "8:1", "4:1", "21:9", "16:9", "3:2", "5:4", "4:3", "1:1", "3:4", "4:5", "2:3", "9:16", "1:4", "1:8"]}
            onChange={setPreset}
          />
          <div className="mx-1 h-5 w-px bg-white/10" />
          <Button
            onClick={sendMessage}
            size="icon"
            aria-label="发送扩图描述"
            title="发送扩图描述"
            className="ml-auto h-9 w-9 rounded-full bg-white text-black hover:bg-white/90"
          >
            <Send className="h-3.5 w-3.5" />
          </Button>
        </div>
        <div className="mt-1 flex items-center justify-center text-[10px] text-white/25">Ctrl / ⌘ + Enter 发送消息</div>
      </section>
    </div>,
    document.body,
  );
}
