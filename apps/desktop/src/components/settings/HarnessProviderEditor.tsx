import { useRef, useState, type FormEvent } from "react";
import { Upload } from "lucide-react";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";
import { ProviderLogo } from "./ProviderLogo";
import {
  HARNESS_BALANCE_KINDS, HARNESS_PROTOCOLS,
  type HarnessBalanceKind, type HarnessProtocol, type HarnessProviderInput, type HarnessProviderView,
} from "../../harnessModelsApi";

export const PROTOCOL_LABEL: Record<HarnessProtocol, string> = {
  responses: "Responses", chat_completions: "Chat Completions", messages: "Messages (Anthropic)",
};
const BALANCE_LABEL: Record<HarnessBalanceKind, string> = {
  auto: "自动探测", none: "不查询", official: "官方接口", new_api: "New API", sub2api: "sub2api",
};

type Props = {
  provider: HarnessProviderView | null;
  onSave: (input: HarnessProviderInput) => Promise<void>;
  onCancel: () => void;
};

export function HarnessProviderEditor({ provider, onSave, onCancel }: Props) {
  const [draft, setDraft] = useState<HarnessProviderInput>(() => ({
    id: provider?.id ?? null, channel: provider?.channel ?? "", logo: provider?.logo ?? "",
    websiteUrl: provider?.websiteUrl ?? "", modelUrl: provider?.modelUrl ?? "",
    protocol: provider?.protocol ?? "chat_completions", balanceKind: provider?.balanceKind ?? "auto",
    enabled: provider?.enabled ?? true, apiKey: null,
  }));
  const [saving, setSaving] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const update = <K extends keyof HarnessProviderInput>(key: K, value: HarnessProviderInput[K]) =>
    setDraft(current => ({ ...current, [key]: value }));

  const upload = async (file: File | undefined) => {
    if (!file) return;
    setError(null);
    if (!["image/png", "image/jpeg", "image/webp", "image/gif"].includes(file.type) || file.size > 1_048_576) {
      setError("请选择 1 MB 以内的 PNG、JPEG、WebP 或 GIF 图片");
      return;
    }
    setUploading(true);
    try {
      const logo = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => typeof reader.result === "string" ? resolve(reader.result) : reject(new Error("无法读取图片"));
        reader.onerror = () => reject(new Error("无法读取图片"));
        reader.readAsDataURL(file);
      });
      await new Promise<void>((resolve, reject) => {
        const image = new Image();
        image.onload = () => resolve();
        image.onerror = () => reject(new Error("图片无法解码，请重新选择"));
        image.src = logo;
      });
      update("logo", logo);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setUploading(false);
    }
  };

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (saving || uploading) return;
    setSaving(true);
    setError(null);
    try {
      await onSave({ ...draft, channel: draft.channel.trim(), modelUrl: draft.modelUrl.trim(),
        websiteUrl: draft.websiteUrl.trim(), apiKey: draft.apiKey?.trim() || null });
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setSaving(false);
    }
  };

  return (
    <form onSubmit={save} className="flex min-h-0 flex-col">
      <div className="space-y-5 overflow-y-auto px-6 pb-6">
        {error && <div role="alert" className="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700">{error}</div>}
        <div className="flex items-end gap-4">
          <div className="flex shrink-0 flex-col gap-2">
            <Label>供应商图标</Label>
            <Button variant="ghost" aria-label="上传供应商图标" disabled={saving || uploading}
              className="group relative size-[76px] rounded-2xl p-0" onClick={() => fileInput.current?.click()}>
              <ProviderLogo logo={draft.logo} name={draft.channel} className="size-full rounded-2xl" />
              <span className="absolute inset-0 flex items-center justify-center rounded-2xl bg-black/40 text-white opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"><Upload className="size-5" /></span>
            </Button>
            <input ref={fileInput} type="file" accept="image/png,image/jpeg,image/webp,image/gif" className="hidden" aria-label="图标图片文件"
              onChange={event => { const file = event.target.files?.[0]; event.target.value = ""; void upload(file); }} />
          </div>
          <div className="flex min-w-0 flex-1 flex-col gap-2 pb-0.5">
            <Label htmlFor="harness-channel">供应商名称</Label>
            <Input id="harness-channel" required value={draft.channel} disabled={saving} onChange={event => update("channel", event.target.value)} placeholder="例如 DeepSeek" />
          </div>
        </div>
        <div className="flex flex-col gap-2">
          <Label htmlFor="harness-url">API 地址</Label>
          <Input id="harness-url" type="url" required value={draft.modelUrl} disabled={saving} onChange={event => update("modelUrl", event.target.value)} placeholder="https://api.example.com/v1" />
        </div>
        <div className="grid grid-cols-2 gap-4">
          <div className="flex flex-col gap-2">
            <Label htmlFor="harness-protocol">协议</Label>
            <Select value={draft.protocol} onValueChange={value => update("protocol", value as HarnessProtocol)}>
              <SelectTrigger className="rounded-lg bg-hover-bg" id="harness-protocol" disabled={saving}><SelectValue placeholder={PROTOCOL_LABEL[draft.protocol]} /></SelectTrigger>
              <SelectContent>{HARNESS_PROTOCOLS.map(value => <SelectItem key={value} value={value}>{PROTOCOL_LABEL[value]}</SelectItem>)}</SelectContent>
            </Select>
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="harness-balance">余额查询</Label>
            <Select value={draft.balanceKind} onValueChange={value => update("balanceKind", value as HarnessBalanceKind)}>
              <SelectTrigger className="rounded-lg bg-hover-bg" id="harness-balance" disabled={saving}><SelectValue placeholder={BALANCE_LABEL[draft.balanceKind]} /></SelectTrigger>
              <SelectContent>{HARNESS_BALANCE_KINDS.map(value => <SelectItem key={value} value={value}>{BALANCE_LABEL[value]}</SelectItem>)}</SelectContent>
            </Select>
          </div>
        </div>
        <div className="flex flex-col gap-2">
          <Label htmlFor="harness-key">API Key</Label>
          <Input id="harness-key" type="password" autoComplete="new-password" value={draft.apiKey ?? ""} disabled={saving}
            onChange={event => update("apiKey", event.target.value)} placeholder={provider?.hasApiKey ? "已保存，留空保持不变" : "输入 API Key"} />
        </div>
        <div className="flex flex-col gap-2">
          <Label htmlFor="harness-website">官网</Label>
          <Input id="harness-website" type="url" value={draft.websiteUrl} disabled={saving} onChange={event => update("websiteUrl", event.target.value)} placeholder="https://example.com" />
        </div>
        <Label className="flex items-center gap-2" htmlFor="harness-enabled">
          <Checkbox id="harness-enabled" checked={draft.enabled} disabled={saving} onCheckedChange={checked => update("enabled", checked === true)} />启用此供应商
        </Label>
      </div>
      <div className="flex justify-end gap-2 border-t border-border-theme px-6 py-4">
        <Button variant="outline" disabled={saving || uploading} onClick={onCancel}>取消</Button>
        <Button type="submit" disabled={saving || uploading || !draft.channel.trim() || !draft.modelUrl.trim()}>{saving ? "保存中" : "保存"}</Button>
      </div>
    </form>
  );
}
