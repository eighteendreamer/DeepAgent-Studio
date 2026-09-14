import { useState, useMemo, useEffect, useRef } from "react";
import {
  Activity,
  Bot,
  CheckCircle2,
  CircleAlert,
  Copy,
  Cpu,
  Eye,
  EyeOff,
  Globe,
  ImagePlus,
  ListChecks,
  Pencil,
  Plus,
  RefreshCw,
  Rocket,
  RotateCcw,
  Search,
  Server,
  Shield,
  Settings2,
  Sparkles,
  Trash2,
  X,
  XCircle,
  Zap,
  Check,
  ChevronDown,
} from "lucide-react";
import { Button } from "../../../../components/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from "../../../../components/shadcn/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "../../../../components/shadcn/dropdown-menu";
import { Input } from "../../../../components/shadcn/input";
import { Label } from "../../../../components/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../../../../components/shadcn/select";
import { cn } from "../../../../components/shadcn/utils";
import { ToggleSwitch } from "../../../../components/ui/ToggleSwitch";
import {
  useCanvasSettingsStore,
  type ModelProtocol,
  type ModelProvider,
  type ModelScenario,
} from "../../store/canvasSettingsStore";

// ============================================================================
// 图片压缩与处理工具函数
// ============================================================================

function processImageFile(file: File, callback: (dataUrl: string) => void) {
  if (!file.type.startsWith("image/")) return;
  const reader = new FileReader();
  reader.onload = () => {
    const result = reader.result as string;
    const img = new Image();
    img.onload = () => {
      const canvas = document.createElement("canvas");
      const maxDim = 128;
      let w = img.width;
      let h = img.height;
      if (w > h) {
        if (w > maxDim) {
          h = Math.round((h * maxDim) / w);
          w = maxDim;
        }
      } else {
        if (h > maxDim) {
          w = Math.round((w * maxDim) / h);
          h = maxDim;
        }
      }
      canvas.width = w;
      canvas.height = h;
      const ctx = canvas.getContext("2d");
      if (ctx) {
        ctx.drawImage(img, 0, 0, w, h);
        callback(canvas.toDataURL("image/png"));
      } else {
        callback(result);
      }
    };
    img.onerror = () => callback(result);
    img.src = result;
  };
  reader.readAsDataURL(file);
}

export function getEffectiveLogo(provider: { name?: string; logo?: string; baseUrl?: string }): string {
  if (provider.logo && provider.logo.trim()) {
    return provider.logo.trim();
  }
  const name = (provider.name || "").toLowerCase();
  const url = (provider.baseUrl || "").toLowerCase();
  if (name.includes("deepseek") || url.includes("deepseek")) return "deepseek";
  if (name.includes("openai") || name.includes("gpt") || url.includes("openai")) return "openai";
  if (name.includes("claude") || name.includes("anthropic") || url.includes("anthropic")) return "anthropic";
  if (name.includes("gemini") || name.includes("google") || url.includes("generativelanguage")) return "gemini";
  if (name.includes("silicon") || name.includes("硅基") || url.includes("siliconflow")) return "siliconflow";
  if (name.includes("ollama") || url.includes("11434")) return "ollama";
  if (name.includes("kimi") || name.includes("moonshot") || url.includes("moonshot")) return "moonshot";
  if (name.includes("智谱") || name.includes("glm") || url.includes("bigmodel")) return "zhipu";
  if (name.includes("千问") || name.includes("qwen") || url.includes("dashscope")) return "qwen";
  if (name.includes("azure")) return "azure";
  return "server";
}

export function ProviderLogo({
  logo,
  name,
  size = "md",
  className = "",
}: {
  logo?: string;
  name?: string;
  size?: "xs" | "sm" | "md" | "lg" | "xl";
  className?: string;
}) {
  const effective = getEffectiveLogo({ logo, name });

  const sizeClasses = {
    xs: "h-5 w-5 rounded text-[10px]",
    sm: "h-7 w-7 rounded-lg text-xs",
    md: "h-9 w-9 rounded-xl text-sm",
    lg: "h-11 w-11 rounded-xl text-base",
    xl: "h-14 w-14 rounded-2xl text-xl",
  }[size];

  const iconSizes = {
    xs: 11,
    sm: 13,
    md: 16,
    lg: 20,
    xl: 26,
  }[size];

  // 自定义图片链接
  if (
    effective.startsWith("http://") ||
    effective.startsWith("https://") ||
    effective.startsWith("data:image/")
  ) {
    return (
      <div
        className={`relative flex shrink-0 items-center justify-center overflow-hidden border border-white/10 bg-white/[0.04] p-1 ${sizeClasses} ${className}`}
      >
        <img
          src={effective}
          alt={name || "logo"}
          className="h-full w-full object-contain"
          onError={(e) => {
            (e.target as HTMLElement).style.display = "none";
          }}
        />
      </div>
    );
  }

  // 自定义 Emoji
  if (/\p{Emoji}/u.test(effective) && effective.length <= 4) {
    return (
      <div
        className={`flex shrink-0 items-center justify-center border border-white/10 bg-white/[0.04] select-none ${sizeClasses} ${className}`}
      >
        <span>{effective}</span>
      </div>
    );
  }

  switch (effective) {
    case "deepseek":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-[#339CFF]/30 bg-[#339CFF]/15 text-[#339CFF] shadow-[0_0_12px_rgba(51,156,255,0.15)] ${sizeClasses} ${className}`}
          title="DeepSeek"
        >
          <svg viewBox="0 0 24 24" fill="none" className="w-[62%] h-[62%]">
            <path
              d="M3.5 13.5C3.5 13.5 5 10 9 10C13 10 14.5 13 18 13C20.5 13 21.5 11 21.5 11M16.5 7.5C18 8.5 19.5 8 20.5 7M7.5 6.5C6 6.5 4.5 7.5 3.5 9M3.5 13.5C3.5 16.5 6.5 19 10.5 19C15 19 18 17 19.5 17C21 17 21.5 17.8 22 18.5"
              stroke="currentColor"
              strokeWidth="2.2"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
            <circle cx="16" cy="6.5" r="1.3" fill="currentColor" />
          </svg>
        </div>
      );
    case "openai":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-[#10A37F]/30 bg-[#10A37F]/15 text-[#10A37F] shadow-[0_0_12px_rgba(16,163,127,0.15)] ${sizeClasses} ${className}`}
          title="OpenAI"
        >
          <svg viewBox="0 0 24 24" fill="none" className="w-[62%] h-[62%]">
            <path
              d="M20.5 10.5C20.2 8.7 18.9 7.4 17.2 7.1C16.9 5.3 15.3 4 13.5 4C12.4 4 11.4 4.5 10.8 5.3C10.2 5 9.4 4.9 8.7 5.1C7 5.6 5.8 7.1 5.8 8.9C4.3 9.4 3.3 10.8 3.3 12.5C3.3 14.3 4.4 15.8 6 16.3C6.3 18.1 7.6 19.4 9.3 19.7C9.6 21.5 11.2 22.8 13 22.8C14.1 22.8 15.1 22.3 15.7 21.5C16.3 21.8 17.1 21.9 17.8 21.7C19.5 21.2 20.7 19.7 20.7 17.9C22.2 17.4 23.2 16 23.2 14.3C23.2 12.5 22.1 11 20.5 10.5Z"
              stroke="currentColor"
              strokeWidth="1.8"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </div>
      );
    case "anthropic":
    case "claude":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-[#D97757]/30 bg-[#D97757]/15 text-[#D97757] shadow-[0_0_12px_rgba(217,119,87,0.15)] ${sizeClasses} ${className}`}
          title="Anthropic Claude"
        >
          <svg viewBox="0 0 24 24" fill="currentColor" className="w-[60%] h-[60%]">
            <path d="M13.8 4L21 19.5H16.8L15.3 16.2H8.7L7.2 19.5H3L10.2 4H13.8ZM12 8.8L9.9 13.4H14.1L12 8.8Z" />
          </svg>
        </div>
      );
    case "gemini":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-[#4285F4]/30 bg-[#4285F4]/15 text-[#4285F4] shadow-[0_0_12px_rgba(66,133,244,0.15)] ${sizeClasses} ${className}`}
          title="Google Gemini"
        >
          <svg viewBox="0 0 24 24" fill="none" className="w-[64%] h-[64%]">
            <path
              d="M12 2C12 7.52 7.52 12 2 12C7.52 12 12 16.48 12 22C12 16.48 16.48 12 22 12C16.48 12 12 7.52 12 2Z"
              fill="url(#gemini-logo-grad)"
            />
            <defs>
              <linearGradient id="gemini-logo-grad" x1="2" y1="2" x2="22" y2="22" gradientUnits="userSpaceOnUse">
                <stop stopColor="#4285F4" />
                <stop offset="0.5" stopColor="#9B72CB" />
                <stop offset="1" stopColor="#D96570" />
              </linearGradient>
            </defs>
          </svg>
        </div>
      );
    case "siliconflow":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-[#8B5CF6]/30 bg-[#8B5CF6]/15 text-[#8B5CF6] shadow-[0_0_12px_rgba(139,92,246,0.15)] ${sizeClasses} ${className}`}
          title="SiliconFlow 硅基流动"
        >
          <Cpu size={iconSizes} strokeWidth={2} />
        </div>
      );
    case "ollama":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-white/25 bg-white/10 text-white shadow-[0_0_12px_rgba(255,255,255,0.1)] ${sizeClasses} ${className}`}
          title="Ollama (本地)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" className="w-[62%] h-[62%]">
            <path d="M7 21v-4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v4M15 15V8a3 3 0 0 0-3-3H9M7 5l2 3M15 5l-2 3" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
            <circle cx="10" cy="9" r="1" fill="currentColor" />
            <circle cx="14" cy="9" r="1" fill="currentColor" />
          </svg>
        </div>
      );
    case "moonshot":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-[#38BDF8]/30 bg-[#38BDF8]/15 text-[#38BDF8] shadow-[0_0_12px_rgba(56,189,248,0.15)] ${sizeClasses} ${className}`}
          title="Moonshot Kimi"
        >
          <svg viewBox="0 0 24 24" fill="none" className="w-[60%] h-[60%]">
            <path
              d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"
              stroke="#38BDF8"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              fill="#38BDF8"
              fillOpacity="0.25"
            />
            <circle cx="18" cy="6" r="1.5" fill="#FACC15" />
          </svg>
        </div>
      );
    case "zhipu":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-[#2563EB]/30 bg-[#2563EB]/15 text-[#2563EB] shadow-[0_0_12px_rgba(37,99,235,0.15)] ${sizeClasses} ${className}`}
          title="智谱 GLM"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="#3B82F6" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="w-[62%] h-[62%]">
            <path d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5" />
          </svg>
        </div>
      );
    case "qwen":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-[#F97316]/30 bg-[#F97316]/15 text-[#F97316] shadow-[0_0_12px_rgba(249,115,22,0.15)] ${sizeClasses} ${className}`}
          title="通义千问"
        >
          <svg viewBox="0 0 24 24" fill="none" className="w-[62%] h-[62%]">
            <circle cx="12" cy="12" r="8" stroke="#F97316" strokeWidth="2" strokeDasharray="3 3" />
            <circle cx="12" cy="12" r="4" fill="#F97316" />
          </svg>
        </div>
      );
    case "azure":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-[#0078D4]/30 bg-[#0078D4]/15 text-[#0078D4] ${sizeClasses} ${className}`}
          title="Azure OpenAI"
        >
          <svg viewBox="0 0 24 24" fill="currentColor" className="w-[62%] h-[62%]">
            <path d="M19.35 10.04C18.67 6.59 15.64 4 12 4 9.11 4 6.6 5.64 5.35 8.04 2.34 8.36 0 10.91 0 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96z" />
          </svg>
        </div>
      );
    case "bot":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-emerald-500/30 bg-emerald-500/15 text-emerald-400 ${sizeClasses} ${className}`}
        >
          <Bot size={iconSizes} />
        </div>
      );
    case "zap":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-amber-500/30 bg-amber-500/15 text-amber-400 ${sizeClasses} ${className}`}
        >
          <Zap size={iconSizes} />
        </div>
      );
    case "globe":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-cyan-500/30 bg-cyan-500/15 text-cyan-400 ${sizeClasses} ${className}`}
        >
          <Globe size={iconSizes} />
        </div>
      );
    case "sparkles":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-pink-500/30 bg-pink-500/15 text-pink-400 ${sizeClasses} ${className}`}
        >
          <Sparkles size={iconSizes} />
        </div>
      );
    case "rocket":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-purple-500/30 bg-purple-500/15 text-purple-400 ${sizeClasses} ${className}`}
        >
          <Rocket size={iconSizes} />
        </div>
      );
    case "shield":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-indigo-500/30 bg-indigo-500/15 text-indigo-400 ${sizeClasses} ${className}`}
        >
          <Shield size={iconSizes} />
        </div>
      );
    case "cpu":
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-[#8B5CF6]/30 bg-[#8B5CF6]/15 text-[#8B5CF6] ${sizeClasses} ${className}`}
        >
          <Cpu size={iconSizes} />
        </div>
      );
    default:
      return (
        <div
          className={`flex shrink-0 items-center justify-center border border-white/10 bg-white/[0.05] text-white/70 ${sizeClasses} ${className}`}
        >
          <Server size={iconSizes} />
        </div>
      );
  }
}

interface ProviderTemplate {
  name: string;
  protocol: ModelProtocol;
  baseUrl: string;
  tag: string;
  logo?: string;
  defaultModels: Array<{ id: string; name: string; description: string }>;
}

const TEMPLATES: ProviderTemplate[] = [
  {
    name: "DeepSeek 官方",
    protocol: "openai",
    baseUrl: "https://api.deepseek.com/v1",
    tag: "DeepSeek",
    logo: "deepseek",
    defaultModels: [
      { id: "deepseek-chat", name: "DeepSeek V3", description: "通用对话与全能编码 (128K 上下文)" },
      { id: "deepseek-reasoner", name: "DeepSeek R1", description: "深度推理与高难度逻辑思考 (64K)" },
    ],
  },
  {
    name: "OpenAI 官方",
    protocol: "openai",
    baseUrl: "https://api.openai.com/v1",
    tag: "OpenAI",
    logo: "openai",
    defaultModels: [
      { id: "gpt-4o", name: "GPT-4o", description: "全模态高智力旗舰 (128K)" },
      { id: "gpt-4o-mini", name: "GPT-4o Mini", description: "高速轻量经济模型 (128K)" },
      { id: "o3-mini", name: "o3-mini", description: "高效科学推理与数学 (200K)" },
    ],
  },
  {
    name: "Anthropic Claude",
    protocol: "anthropic",
    baseUrl: "https://api.anthropic.com/v1",
    tag: "Claude",
    logo: "anthropic",
    defaultModels: [
      { id: "claude-3-7-sonnet-20250219", name: "Claude 3.7 Sonnet", description: "混合推理与编码旗舰 (200K)" },
      { id: "claude-3-5-sonnet-20241022", name: "Claude 3.5 Sonnet", description: "经典代码与指令遵循模型 (200K)" },
      { id: "claude-3-5-haiku-20241022", name: "Claude 3.5 Haiku", description: "极速日常响应轻量版 (200K)" },
    ],
  },
  {
    name: "SiliconFlow 硅基流动",
    protocol: "openai",
    baseUrl: "https://api.siliconflow.cn/v1",
    tag: "SiliconFlow",
    logo: "siliconflow",
    defaultModels: [
      { id: "deepseek-ai/DeepSeek-V3", name: "DeepSeek-V3 (云托管)", description: "满血高并发通用模型" },
      { id: "deepseek-ai/DeepSeek-R1", name: "DeepSeek-R1 (云托管)", description: "满血深度推理模型" },
    ],
  },
  {
    name: "Ollama (本地)",
    protocol: "openai",
    baseUrl: "http://localhost:11434/v1",
    tag: "Ollama",
    logo: "ollama",
    defaultModels: [
      { id: "deepseek-r1:8b", name: "DeepSeek-R1 8B", description: "本地离线推理轻量版" },
      { id: "qwen2.5-coder:7b", name: "Qwen 2.5 Coder 7B", description: "本地高性价比代码模型" },
    ],
  },
];

const PROTOCOLS: { value: ModelProtocol; label: string }[] = [
  { value: "openai", label: "OpenAI" },
  { value: "anthropic", label: "Anthropic" },
  { value: "gemini", label: "Gemini" },
];

const MODEL_SCENARIO_OPTIONS: { value: ModelScenario; label: string }[] = [
  { value: "text", label: "文本" },
  { value: "image_generation", label: "生图" },
  { value: "video_generation", label: "视频生成" },
  { value: "speech_to_text", label: "语音转文字" },
  { value: "text_to_speech", label: "文字转语音" },
];

function scenarioLabels(scenarios?: ModelScenario[]): string[] {
  const values = scenarios ?? [];
  return MODEL_SCENARIO_OPTIONS.filter((option) => values.includes(option.value)).map(
    (option) => option.label,
  );
}

type ModelSettingsMode = "providers" | "models";

interface PingStatus {
  testing: boolean;
  ok?: boolean;
  message?: string;
  latency?: number;
}

export function ModelSettingsTab() {
  const providers = useCanvasSettingsStore((s) => s.providers);
  const addProvider = useCanvasSettingsStore((s) => s.addProvider);
  const updateProvider = useCanvasSettingsStore((s) => s.updateProvider);
  const removeProvider = useCanvasSettingsStore((s) => s.removeProvider);
  const toggleProviderEnabled = useCanvasSettingsStore((s) => s.toggleProviderEnabled);
  const updateProviderModels = useCanvasSettingsStore((s) => s.updateProviderModels);
  const addModelToProvider = useCanvasSettingsStore((s) => s.addModelToProvider);
  const setModelScenarios = useCanvasSettingsStore((s) => s.setModelScenarios);
  const toggleModelEnabled = useCanvasSettingsStore((s) => s.toggleModelEnabled);
  const removeModelFromProvider = useCanvasSettingsStore((s) => s.removeModelFromProvider);

  // 模式切换：AI 服务商 vs 模型管理
  const [mode, setMode] = useState<ModelSettingsMode>("providers");

  // 选中的供应商
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(
    providers[0]?.id ?? null,
  );

  // 搜索关键词
  const [providerSearch, setProviderSearch] = useState("");
  const [modelSearch, setModelSearch] = useState("");
  const [globalProviderFilter, setGlobalProviderFilter] = useState<string>("all");
  const [globalScenarioFilter, setGlobalScenarioFilter] = useState<ModelScenario | "all">("all");

  // API Key 显隐状态
  const [showApiKey, setShowApiKey] = useState<Record<string, boolean>>({});
  const [copiedKey, setCopiedKey] = useState(false);

  // 连接测试状态
  const [pingStatus, setPingStatus] = useState<Record<string, PingStatus>>({});

  // 弹窗状态
  const [addProviderDialogOpen, setAddProviderDialogOpen] = useState(false);
  const [providerForm, setProviderForm] = useState({
    name: "",
    protocol: "openai" as ModelProtocol,
    baseUrl: "",
    apiKey: "",
    logo: "",
  });
  const fileInputRef = useRef<HTMLInputElement>(null);
  const changeLogoFileInputRef = useRef<HTMLInputElement>(null);

  const [changeLogoDialogOpen, setChangeLogoDialogOpen] = useState(false);

  const [addModelDialogOpen, setAddModelDialogOpen] = useState(false);
  const [addModelTargetProviderId, setAddModelTargetProviderId] = useState<string | null>(null);
  const [newModelForm, setNewModelForm] = useState({ id: "", name: "", description: "" });

  const [scenarioDialogTarget, setScenarioDialogTarget] = useState<{
    providerId: string;
    modelId: string;
    modelName: string;
  } | null>(null);
  const [draftScenarios, setDraftScenarios] = useState<ModelScenario[]>([]);

  const [deleteConfirmProvider, setDeleteConfirmProvider] = useState<ModelProvider | null>(null);

  // 保持当前选中的供应商有效
  useEffect(() => {
    if (providers.length === 0) {
      setSelectedProviderId(null);
      return;
    }
    if (!selectedProviderId || !providers.some((p) => p.id === selectedProviderId)) {
      setSelectedProviderId(providers[0].id);
    }
  }, [providers, selectedProviderId]);

  const selectedProvider = useMemo(
    () => providers.find((p) => p.id === selectedProviderId),
    [providers, selectedProviderId],
  );

  // 过滤供应商
  const filteredProviders = useMemo(() => {
    const q = providerSearch.trim().toLowerCase();
    if (!q) return providers;
    return providers.filter(
      (p) =>
        p.name.toLowerCase().includes(q) ||
        p.baseUrl.toLowerCase().includes(q) ||
        p.protocol.toLowerCase().includes(q),
    );
  }, [providerSearch, providers]);

  // 当前选中供应商的模型列表
  const currentModels = useMemo(() => {
    if (!selectedProvider) return [];
    const models = selectedProvider.models ?? [];
    const q = modelSearch.trim().toLowerCase();
    if (!q) return models;
    return models.filter(
      (m) =>
        m.id.toLowerCase().includes(q) ||
        m.name.toLowerCase().includes(q) ||
        (m.description ?? "").toLowerCase().includes(q),
    );
  }, [selectedProvider, modelSearch]);

  // 全局所有模型
  const allModelsWithProvider = useMemo(() => {
    return providers.flatMap((p) =>
      (p.models ?? []).map((m) => ({
        ...m,
        provider: p,
      })),
    );
  }, [providers]);

  // 全局模型筛选
  const filteredGlobalModels = useMemo(() => {
    let list = allModelsWithProvider;
    if (globalProviderFilter !== "all") {
      list = list.filter((item) => item.provider.id === globalProviderFilter);
    }
    if (globalScenarioFilter !== "all") {
      list = list.filter((item) => (item.scenarios ?? []).includes(globalScenarioFilter));
    }
    const q = modelSearch.trim().toLowerCase();
    if (!q) return list;
    return list.filter(
      (item) =>
        item.name.toLowerCase().includes(q) ||
        item.id.toLowerCase().includes(q) ||
        item.provider.name.toLowerCase().includes(q) ||
        (item.description ?? "").toLowerCase().includes(q),
    );
  }, [allModelsWithProvider, globalProviderFilter, globalScenarioFilter, modelSearch]);

  const openScenarioDialog = (providerId: string, modelId: string, modelName: string, scenarios?: ModelScenario[]) => {
    setScenarioDialogTarget({ providerId, modelId, modelName });
    setDraftScenarios([...(scenarios ?? [])]);
  };

  const toggleDraftScenario = (scenario: ModelScenario) => {
    setDraftScenarios((current) =>
      current.includes(scenario)
        ? current.filter((value) => value !== scenario)
        : [...current, scenario],
    );
  };

  const handleSaveScenarios = () => {
    if (!scenarioDialogTarget) return;
    setModelScenarios(
      scenarioDialogTarget.providerId,
      scenarioDialogTarget.modelId,
      draftScenarios,
    );
    setScenarioDialogTarget(null);
  };

  const totalEnabledModels = useMemo(
    () => allModelsWithProvider.filter((m) => m.enabled).length,
    [allModelsWithProvider],
  );

  // 打开添加供应商弹窗
  const handleOpenAddProviderDialog = (tmpl?: ProviderTemplate) => {
    setProviderForm({
      name: tmpl?.name ?? "",
      protocol: tmpl?.protocol ?? "openai",
      baseUrl: tmpl?.baseUrl ?? "",
      apiKey: "",
      logo: tmpl?.logo ?? "",
    });
    setAddProviderDialogOpen(true);
  };

  // 确认添加供应商
  const handleConfirmAddProvider = () => {
    const trimmedName = providerForm.name.trim();
    const trimmedUrl = providerForm.baseUrl.trim();
    if (!trimmedName || !trimmedUrl) return;

    // 寻找预设推荐模型
    const tmpl = TEMPLATES.find((t) => t.name === trimmedName);
    const defaultModels = (tmpl?.defaultModels ?? []).map((m) => ({
      ...m,
      enabled: true,
    }));

    const newId = addProvider({
      name: trimmedName,
      protocol: providerForm.protocol,
      baseUrl: trimmedUrl,
      apiKey: providerForm.apiKey.trim(),
      logo: providerForm.logo.trim() || undefined,
      enabled: true,
      models: defaultModels,
    });

    setSelectedProviderId(newId);
    setAddProviderDialogOpen(false);
  };

  // 测试连通性
  const handleTestConnection = async (provider: ModelProvider) => {
    setPingStatus((s) => ({
      ...s,
      [provider.id]: { testing: true, message: "正在测试连接..." },
    }));

    const start = performance.now();
    try {
      const trimmedUrl = provider.baseUrl.trim().replace(/\/+$/, "");
      const testUrl = `${trimmedUrl}/models`;
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), 6000);
      const headers: Record<string, string> = {};
      if (provider.apiKey.trim()) {
        if (provider.protocol === "anthropic") {
          headers["x-api-key"] = provider.apiKey.trim();
          headers["anthropic-version"] = "2023-06-01";
        } else {
          headers["Authorization"] = `Bearer ${provider.apiKey.trim()}`;
        }
      }

      const res = await fetch(testUrl, {
        method: "GET",
        headers,
        signal: controller.signal,
      });
      clearTimeout(timeoutId);
      const ms = Math.round(performance.now() - start);

      if (res.ok) {
        setPingStatus((s) => ({
          ...s,
          [provider.id]: { testing: false, ok: true, message: `连接正常 (${res.status} OK · ${ms}ms)`, latency: ms },
        }));
      } else {
        setPingStatus((s) => ({
          ...s,
          [provider.id]: { testing: false, ok: false, message: `服务返回状态 ${res.status} (${res.statusText} · ${ms}ms)`, latency: ms },
        }));
      }
    } catch (err: any) {
      const ms = Math.round(performance.now() - start);
      const isTimeout = err.name === "AbortError";
      setPingStatus((s) => ({
        ...s,
        [provider.id]: {
          testing: false,
          ok: false,
          message: isTimeout ? "连接超时 (>6000ms)" : `连接受阻: ${err.message || "网络异常"} (${ms}ms)`,
          latency: ms,
        },
      }));
    }
  };

  // 一键同步/获取推荐模型
  const handleFetchPresetModels = (provider: ModelProvider) => {
    const matched = TEMPLATES.find(
      (t) =>
        provider.baseUrl.toLowerCase().includes(new URL(t.baseUrl).hostname.toLowerCase()) ||
        provider.name.toLowerCase().includes(t.tag.toLowerCase()),
    );
    if (matched) {
      const existingIds = new Set((provider.models ?? []).map((m) => m.id));
      const newModels = [
        ...(provider.models ?? []),
        ...matched.defaultModels.filter((m) => !existingIds.has(m.id)).map((m) => ({ ...m, enabled: true })),
      ];
      updateProviderModels(provider.id, newModels);
    } else {
      const genericOpenAi = [
        { id: "gpt-4o", name: "GPT-4o", description: "通用全模态主力模型", enabled: true },
        { id: "gpt-4o-mini", name: "GPT-4o Mini", description: "轻量高速经济模型", enabled: true },
      ];
      const existingIds = new Set((provider.models ?? []).map((m) => m.id));
      const newModels = [
        ...(provider.models ?? []),
        ...genericOpenAi.filter((m) => !existingIds.has(m.id)),
      ];
      updateProviderModels(provider.id, newModels);
    }
  };

  // 批量启用/禁用模型
  const handleBatchToggleModels = (providerId: string, enableAll: boolean) => {
    const provider = providers.find((p) => p.id === providerId);
    if (!provider) return;
    const updated = (provider.models ?? []).map((m) => ({ ...m, enabled: enableAll }));
    updateProviderModels(providerId, updated);
  };

  // 复制 Key
  const handleCopyKey = (key: string) => {
    if (!key) return;
    navigator.clipboard.writeText(key);
    setCopiedKey(true);
    setTimeout(() => setCopiedKey(false), 1500);
  };

  // 提交新建模型
  const handleConfirmAddModel = () => {
    if (!addModelTargetProviderId || !newModelForm.id.trim()) return;
    addModelToProvider(addModelTargetProviderId, {
      id: newModelForm.id.trim(),
      name: newModelForm.name.trim() || newModelForm.id.trim(),
      description: newModelForm.description.trim(),
      enabled: true,
    });
    setNewModelForm({ id: "", name: "", description: "" });
    setAddModelDialogOpen(false);
  };

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-[#0d0d0d] text-white select-none">
      {/* 顶部全局模式切换区 (固定在右上角) */}
      <div className="flex shrink-0 items-center justify-between border-b border-white/[0.08] px-6 py-3 bg-white/[0.015]">
        <div className="flex items-center gap-2.5">
          <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-[#339CFF]/15 text-[#339CFF]">
            <Server size={14} />
          </div>
          <div>
            <div className="text-[13px] font-semibold text-white/90">
              {mode === "providers" ? "AI 服务商配置" : "全量模型资产管理"}
            </div>
            <div className="text-[11px] text-white/40">
              {mode === "providers"
                ? "配置服务商连接凭据与可用模型列表"
                : "全景查看并统一控制所有已启用的模型服务"}
            </div>
          </div>
        </div>

        {/* 顶部右侧：模式切换控制器 */}
        <div className="flex items-center rounded-lg p-0.5 border border-white/[0.08] bg-white/[0.03]">
          <button
            type="button"
            onClick={() => setMode("providers")}
            className={`flex items-center gap-1.5 px-3 py-1 rounded-md text-[11px] font-medium transition-all ${
              mode === "providers"
                ? "bg-white/10 text-white shadow-sm font-semibold"
                : "text-white/50 hover:text-white/80"
            }`}
          >
            <Server size={12} />
            AI 服务商
          </button>
          <button
            type="button"
            onClick={() => setMode("models")}
            className={`flex items-center gap-1.5 px-3 py-1 rounded-md text-[11px] font-medium transition-all ${
              mode === "models"
                ? "bg-white/10 text-white shadow-sm font-semibold"
                : "text-white/50 hover:text-white/80"
            }`}
          >
            <Cpu size={12} />
            模型管理
          </button>
        </div>
      </div>

      {/* 主体双栏内容 */}
      <div className="flex min-h-0 min-w-0 flex-1 overflow-hidden">
        {mode === "providers" ? (
          <>
            {/* ============================================================= */}
            {/* 左侧：供应商列表 */}
            {/* ============================================================= */}
            <aside className="flex h-full w-[240px] shrink-0 flex-col border-r border-white/[0.08] bg-white/[0.012]">
              {/* 搜索与添加 */}
              <div className="p-3 border-b border-white/[0.08] space-y-2 shrink-0">
                <div className="flex items-center gap-1.5">
                  <div className="relative flex-1">
                    <Search
                      size={13}
                      className="pointer-events-none absolute left-2.5 top-2 text-white/35"
                    />
                    <Input
                      value={providerSearch}
                      onChange={(e) => setProviderSearch(e.target.value)}
                      placeholder="搜索供应商..."
                      className="h-7 rounded-lg pl-8 text-[11px] bg-white/[0.03] border-white/[0.08] text-white/90 placeholder:text-white/30 focus-visible:ring-1 focus-visible:ring-[#339CFF]"
                    />
                  </div>
                  <button
                    type="button"
                    onClick={() => handleOpenAddProviderDialog()}
                    title="添加 AI 服务商"
                    className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-[#339CFF] text-white hover:bg-[#2563EB] transition-colors shadow-sm"
                  >
                    <Plus size={14} strokeWidth={2.2} />
                  </button>
                </div>

                {/* 快捷添加模板标签 */}
                <div>
                  <div className="text-[10px] font-medium text-white/40 mb-1 flex items-center gap-1">
                    <Sparkles size={10} className="text-[#339CFF]" />
                    推荐预设:
                  </div>
                  <div className="flex flex-wrap gap-1">
                    {TEMPLATES.slice(0, 4).map((tmpl) => (
                      <button
                        key={tmpl.name}
                        type="button"
                        onClick={() => handleOpenAddProviderDialog(tmpl)}
                        className="rounded border border-white/[0.07] bg-white/[0.02] px-1.5 py-0.5 text-[9.5px] text-white/60 hover:border-[#339CFF]/50 hover:bg-[#339CFF]/10 hover:text-white transition-all"
                      >
                        + {tmpl.tag}
                      </button>
                    ))}
                  </div>
                </div>
              </div>

              {/* 供应商列表 */}
              <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain p-2 space-y-1">
                {filteredProviders.length === 0 ? (
                  <div className="flex flex-col items-center justify-center py-12 text-center text-white/40">
                    <Server size={20} className="mb-2 opacity-30" />
                    <div className="text-[11px]">未找到匹配供应商</div>
                    <div
                      role="button"
                      tabIndex={0}
                      onClick={() => handleOpenAddProviderDialog()}
                      className="text-[10px] text-white/40 hover:text-[#339CFF] mt-1 cursor-pointer transition-colors"
                    >
                      点击右上角 [+] 开始配置
                    </div>
                  </div>
                ) : (
                  filteredProviders.map((p) => {
                    const active = selectedProviderId === p.id;
                    const isConfigured = Boolean(p.baseUrl && p.apiKey);
                    const enabledCount = (p.models ?? []).filter((m) => m.enabled).length;
                    const isMasterEnabled = p.enabled !== false;

                    return (
                      <div
                        key={p.id}
                        role="button"
                        tabIndex={0}
                        onClick={() => setSelectedProviderId(p.id)}
                        className={`group relative flex w-full cursor-pointer items-center gap-2 rounded-lg px-2.5 py-2 text-left transition-all border ${
                          active
                            ? "border-[#339CFF]/50 bg-[#339CFF]/10 text-white shadow-sm"
                            : "border-transparent hover:border-white/[0.06] hover:bg-white/[0.03] text-white/70"
                        } ${!isMasterEnabled ? "opacity-50" : ""}`}
                      >
                        <ProviderLogo logo={p.logo} name={p.name} size="sm" />

                        <div className="min-w-0 flex-1">
                          <div className="flex items-center gap-1.5">
                            <span className="truncate text-[12px] font-medium text-white/90">
                              {p.name}
                            </span>
                            <span
                              className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                                !isMasterEnabled
                                  ? "bg-gray-500"
                                  : isConfigured
                                  ? "bg-emerald-400 shadow-[0_0_6px_rgba(52,211,153,0.6)]"
                                  : "bg-amber-400"
                              }`}
                            />
                          </div>

                          <div className="flex items-center gap-1 text-[10px] text-white/40 mt-0.5">
                            <span className="truncate uppercase">{p.protocol}</span>
                            <span>·</span>
                            <span>{enabledCount} 启用</span>
                          </div>
                        </div>
                      </div>
                    );
                  })
                )}
              </div>
            </aside>

            {/* ============================================================= */}
            {/* 右侧：服务商详情与模型列表 (整体不上下滚动，仅模型列表内滚动) */}
            {/* ============================================================= */}
            <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
              {selectedProvider ? (
                <div className="flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden px-6 pt-4 pb-3">
                  {/* 1. 顶部 Header (轻量化无卡片包裹，透明底+底部分割线) */}
                  <div className="shrink-0 pb-3 border-b border-white/[0.06] flex items-center justify-between gap-4">
                    <div className="flex items-center gap-3 min-w-0 flex-1">
                      <div
                        role="button"
                        tabIndex={0}
                        onClick={() => setChangeLogoDialogOpen(true)}
                        title="点击更换服务商 Logo"
                        className="group relative cursor-pointer"
                      >
                        <ProviderLogo
                          logo={selectedProvider.logo}
                          name={selectedProvider.name}
                          size="md"
                          className="transition-transform group-hover:scale-105"
                        />
                        <div className="absolute inset-0 flex items-center justify-center rounded-xl bg-black/60 opacity-0 transition-opacity group-hover:opacity-100">
                          <Pencil size={12} className="text-white" />
                        </div>
                      </div>
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <Input
                            value={selectedProvider.name}
                            onChange={(e) =>
                              updateProvider(selectedProvider.id, { name: e.target.value })
                            }
                            placeholder="供应商名称"
                            className="h-7 w-auto max-w-[260px] rounded-md border-transparent hover:border-white/[0.12] focus:border-[#339CFF] bg-transparent text-[14px] font-semibold text-white/95 px-1.5 -ml-1.5"
                          />
                          <span className="rounded bg-white/[0.06] px-1.5 py-0.5 text-[10px] text-white/50 font-mono uppercase">
                            {selectedProvider.protocol}
                          </span>
                        </div>
                        <div className="text-[11px] text-white/40 px-0.5 flex items-center gap-2 mt-0.5">
                          <span>{(selectedProvider.models ?? []).length} 个模型资产</span>
                          <span>·</span>
                          <span>{selectedProvider.enabled !== false ? "服务商启用中" : "已停用"}</span>
                        </div>
                      </div>
                    </div>

                    {/* 右侧动作 */}
                    <div className="flex items-center gap-2 shrink-0">
                      {/* 连通性测试状态展示 */}
                      {pingStatus[selectedProvider.id] && (
                        <div
                          className={`flex items-center gap-1.5 px-2 py-0.5 rounded-md text-[10px] border ${
                            pingStatus[selectedProvider.id].testing
                              ? "bg-blue-500/10 border-blue-500/30 text-blue-300"
                              : pingStatus[selectedProvider.id].ok
                              ? "bg-emerald-500/10 border-emerald-500/30 text-emerald-300"
                              : "bg-red-500/10 border-red-500/30 text-red-300"
                          }`}
                        >
                          {pingStatus[selectedProvider.id].testing ? (
                            <RefreshCw size={11} className="animate-spin text-blue-400" />
                          ) : pingStatus[selectedProvider.id].ok ? (
                            <CheckCircle2 size={11} className="text-emerald-400" />
                          ) : (
                            <XCircle size={11} className="text-red-400" />
                          )}
                          <span>{pingStatus[selectedProvider.id].message}</span>
                        </div>
                      )}

                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        disabled={
                          !selectedProvider.baseUrl ||
                          pingStatus[selectedProvider.id]?.testing
                        }
                        onClick={() => handleTestConnection(selectedProvider)}
                        className="h-7 rounded-lg border-white/[0.08] bg-white/[0.03] text-[11px] text-white/80 hover:bg-white/[0.07] px-2.5"
                      >
                        <Activity size={12} className="mr-1.5 text-[#339CFF]" />
                        测试连通性
                      </Button>

                      <button
                        type="button"
                        onClick={() => setDeleteConfirmProvider(selectedProvider)}
                        title="删除该供应商"
                        className="flex h-7 w-7 items-center justify-center rounded-lg border border-red-500/20 bg-red-500/10 text-red-400 hover:bg-red-500/20 transition-colors"
                      >
                        <Trash2 size={13} />
                      </button>

                      <div className="flex items-center gap-2 pl-2 border-l border-white/[0.08]">
                        <ToggleSwitch
                          checked={selectedProvider.enabled !== false}
                          onChange={() => toggleProviderEnabled(selectedProvider.id)}
                          size="sm"
                        />
                      </div>
                    </div>
                  </div>

                  {/* 2. 凭证配置区 (轻量化无卡片包裹，透明底+底部分割线) */}
                  <div className="shrink-0 py-3 border-b border-white/[0.06] space-y-2.5">
                    <div className="grid grid-cols-1 md:grid-cols-[1fr_1.2fr_140px] gap-3 items-end">
                      {/* API Key */}
                      <div className="space-y-1">
                        <div className="flex items-center justify-between">
                          <Label className="text-[11px] text-white/50">APIKey</Label>
                          {copiedKey && (
                            <span className="text-[10px] text-emerald-400">已复制!</span>
                          )}
                        </div>
                        <div className="relative flex items-center">
                          <Input
                            type={showApiKey[selectedProvider.id] ? "text" : "password"}
                            value={selectedProvider.apiKey}
                            onChange={(e) =>
                              updateProvider(selectedProvider.id, { apiKey: e.target.value })
                            }
                            placeholder="请输入 APIKey (sk-...)"
                            className="h-8 rounded-lg pr-14 text-[11px] bg-white/[0.03] border-white/[0.08] text-white/90 focus-visible:ring-1 focus-visible:ring-[#339CFF]"
                          />
                          <div className="absolute right-1 flex items-center">
                            <button
                              type="button"
                              onClick={() =>
                                setShowApiKey((s) => ({
                                  ...s,
                                  [selectedProvider.id]: !s[selectedProvider.id],
                                }))
                              }
                              className="p-1 text-white/40 hover:text-white/80 transition-colors"
                              title={showApiKey[selectedProvider.id] ? "隐藏 Key" : "显示 Key"}
                            >
                              {showApiKey[selectedProvider.id] ? (
                                <EyeOff size={12} />
                              ) : (
                                <Eye size={12} />
                              )}
                            </button>
                            <button
                              type="button"
                              onClick={() => handleCopyKey(selectedProvider.apiKey)}
                              className="p-1 text-white/40 hover:text-white/80 transition-colors"
                              title="复制 Key"
                            >
                              <Copy size={12} />
                            </button>
                          </div>
                        </div>
                      </div>

                      {/* API 代理地址 */}
                      <div className="space-y-1">
                        <Label className="text-[11px] text-white/50">API 代理地址 (Base URL)</Label>
                        <Input
                          value={selectedProvider.baseUrl}
                          onChange={(e) =>
                            updateProvider(selectedProvider.id, { baseUrl: e.target.value })
                          }
                          placeholder="如 https://api.deepseek.com/v1"
                          className="h-8 rounded-lg text-[11px] bg-white/[0.03] border-white/[0.08] text-white/90 focus-visible:ring-1 focus-visible:ring-[#339CFF]"
                        />
                      </div>

                      {/* 协议类型 */}
                      <div className="space-y-1">
                        <Label className="text-[11px] text-white/50">协议</Label>
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <button
                              type="button"
                              className="flex h-8 w-full items-center justify-between gap-2 rounded-lg border border-white/[0.08] bg-white/[0.03] px-2.5 text-[11.5px] text-white/90 hover:bg-white/[0.06] transition-colors focus:outline-none focus:ring-1 focus:ring-[#339CFF]"
                            >
                              <span>
                                {PROTOCOLS.find((p) => p.value === selectedProvider.protocol)?.label ||
                                  selectedProvider.protocol}
                              </span>
                              <ChevronDown size={13} className="text-white/40 shrink-0" />
                            </button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent
                            align="start"
                            sideOffset={4}
                            className="!z-[20000] min-w-[130px] rounded-xl border border-white/10 !bg-[#1c1d21] p-1 text-white shadow-2xl"
                          >
                            {PROTOCOLS.map((p) => {
                              const active = selectedProvider.protocol === p.value;
                              return (
                                <DropdownMenuItem
                                  key={p.value}
                                  onClick={() =>
                                    updateProvider(selectedProvider.id, {
                                      protocol: p.value as ModelProtocol,
                                    })
                                  }
                                  className={cn(
                                    "flex items-center justify-between rounded-lg px-2.5 py-1.5 text-[11.5px] cursor-pointer outline-none transition-colors",
                                    active
                                      ? "bg-[#339CFF]/15 text-[#339CFF] font-medium"
                                      : "text-white/80 hover:bg-white/[0.08] hover:text-white focus:bg-white/[0.08] focus:text-white data-[highlighted]:bg-white/[0.08] data-[highlighted]:text-white",
                                  )}
                                >
                                  <span>{p.label}</span>
                                  {active && <Check size={12} className="text-[#339CFF] shrink-0" />}
                                </DropdownMenuItem>
                              );
                            })}
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </div>
                    </div>
                  </div>

                  {/* 3. 模型列表区 (填满剩余高度，仅内部模型列表滚动) */}
                  <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden pt-3">
                    {/* 工具栏 (固定在顶部) */}
                    <div className="shrink-0 space-y-2 pb-2">
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <div className="text-[13px] font-semibold text-white/90 flex items-center gap-2">
                          <ListChecks size={14} className="text-[#339CFF]" />
                          模型列表
                          <span className="rounded-full bg-white/[0.08] px-2 py-0.2 text-[10px] text-white/60">
                            {currentModels.length}
                          </span>
                        </div>

                        <div className="flex items-center gap-1.5">
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            disabled={!currentModels.length}
                            onClick={() => handleBatchToggleModels(selectedProvider.id, true)}
                            className="h-7 rounded-lg border-white/[0.08] bg-white/[0.03] text-[11px] text-white/80 hover:bg-white/[0.07] px-2.5"
                          >
                            全部启用
                          </Button>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            disabled={!currentModels.length}
                            onClick={() => handleBatchToggleModels(selectedProvider.id, false)}
                            className="h-7 rounded-lg border-white/[0.08] bg-white/[0.03] text-[11px] text-white/80 hover:bg-white/[0.07] px-2.5"
                          >
                            全部禁用
                          </Button>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() => handleFetchPresetModels(selectedProvider)}
                            className="h-7 rounded-lg border-white/[0.08] bg-white/[0.03] text-[11px] text-white/80 hover:bg-white/[0.07] px-2.5"
                          >
                            <RefreshCw size={11} className="mr-1 text-[#339CFF]" />
                            获取模型
                          </Button>
                          <Button
                            type="button"
                            size="sm"
                            onClick={() => {
                              setAddModelTargetProviderId(selectedProvider.id);
                              setAddModelDialogOpen(true);
                            }}
                            className="h-7 rounded-lg bg-[#339CFF] px-2.5 text-[11px] font-medium text-white hover:bg-[#2563EB]"
                          >
                            <Plus size={12} className="mr-1" />
                            添加模型
                          </Button>
                        </div>
                      </div>

                      {/* 搜索框 */}
                      <div className="relative">
                        <Search
                          size={13}
                          className="pointer-events-none absolute left-3 top-2 text-white/35"
                        />
                        <Input
                          value={modelSearch}
                          onChange={(e) => setModelSearch(e.target.value)}
                          placeholder="搜索该供应商下的模型名称或 ID..."
                          className="h-7 rounded-lg pl-9 text-[11px] bg-white/[0.03] border-white/[0.08] text-white/90 placeholder:text-white/30"
                        />
                      </div>
                    </div>

                    {/* 真正滚动的模型列表 (Only this area scrolls) */}
                    <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain divide-y divide-white/[0.04] rounded-lg border border-white/[0.06] bg-white/[0.01]">
                      {currentModels.length === 0 ? (
                        <div className="flex flex-col items-center justify-center py-12 px-4 text-center">
                          <Cpu size={22} className="mb-2 text-white/20" />
                          <div className="text-[12px] font-medium text-white/70">
                            暂无模型配置
                          </div>
                          <div className="text-[10px] text-white/40 mt-1">
                            点击上方【获取模型】载入推荐模型清单
                          </div>
                          <Button
                            type="button"
                            size="sm"
                            onClick={() => handleFetchPresetModels(selectedProvider)}
                            className="mt-3 h-7 rounded-lg bg-white/[0.08] text-[11px] text-white hover:bg-white/[0.14]"
                          >
                            <RefreshCw size={11} className="mr-1.5 text-[#339CFF]" />
                            一键载入推荐模型清单
                          </Button>
                        </div>
                      ) : (
                        currentModels.map((model) => (
                          <div
                            key={model.id}
                            className="group flex items-center justify-between gap-3 px-3 py-2 hover:bg-white/[0.03] transition-colors"
                          >
                            <div className="flex items-center gap-2.5 min-w-0 flex-1">
                              <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-[#339CFF]/10 text-[#339CFF]">
                                <Cpu size={13} />
                              </div>
                              <div className="min-w-0 flex-1">
                                <div className="flex items-center gap-2">
                                  <span className="truncate text-[12px] font-medium text-white/90">
                                    {model.name}
                                  </span>
                                  <span className="rounded bg-white/[0.05] px-1.5 py-0.2 text-[9.5px] text-white/50 font-mono">
                                    {model.id}
                                  </span>
                                </div>
                                <div className="truncate text-[10px] text-white/40 mt-0.2">
                                  {model.description || "通用模型服务"}
                                </div>
                                <div className="mt-1 flex min-w-0 items-center gap-1 overflow-hidden">
                                  {scenarioLabels(model.scenarios).length > 0 ? (
                                    scenarioLabels(model.scenarios).map((label) => (
                                      <span
                                        key={label}
                                        className="shrink-0 rounded bg-[#339CFF]/10 px-1.5 py-0.5 text-[9px] text-[#7CC2FF]"
                                      >
                                        {label}
                                      </span>
                                    ))
                                  ) : (
                                    <span className="text-[9px] text-white/25">未设置场景</span>
                                  )}
                                </div>
                              </div>
                            </div>

                            <div className="flex items-center gap-2.5 shrink-0">
                              <button
                                type="button"
                                onClick={() =>
                                  openScenarioDialog(
                                    selectedProvider.id,
                                    model.id,
                                    model.name,
                                    model.scenarios,
                                  )
                                }
                                className="flex items-center gap-1 rounded-md border border-white/[0.08] bg-white/[0.02] px-1.5 py-1 text-[10px] text-white/55 transition-colors hover:border-[#339CFF]/40 hover:bg-[#339CFF]/10 hover:text-[#9DD4FF]"
                                title="设置模型使用场景"
                              >
                                <Settings2 size={11} />
                                设置
                              </button>
                              <button
                                type="button"
                                onClick={() =>
                                  removeModelFromProvider(selectedProvider.id, model.id)
                                }
                                className="opacity-0 group-hover:opacity-100 p-1 text-white/30 hover:text-red-400 transition-all"
                                title="删除模型"
                              >
                                <Trash2 size={12} />
                              </button>
                              <ToggleSwitch
                                checked={model.enabled}
                                onChange={() =>
                                  toggleModelEnabled(selectedProvider.id, model.id)
                                }
                                size="sm"
                              />
                            </div>
                          </div>
                        ))
                      )}
                    </div>
                  </div>
                </div>
              ) : (
                <div className="flex h-full flex-col items-center justify-center py-24 text-center">
                  <Server size={30} className="mb-3 text-white/20" />
                  <div className="text-[13px] font-medium text-white/70">
                    请在左侧选择或添加一个供应商
                  </div>
                  <Button
                    type="button"
                    onClick={() => handleOpenAddProviderDialog()}
                    className="mt-3.5 h-7 rounded-lg bg-[#339CFF] px-3.5 text-[11px] text-white hover:bg-[#2563EB]"
                  >
                    <Plus size={13} className="mr-1" />
                    添加服务商
                  </Button>
                </div>
              )}
            </main>
          </>
        ) : (
          /* ============================================================= */
          /* 模式二：全局模型管理 (整体不滚动，仅表格内滚动) */
          /* ============================================================= */
          <main className="flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden px-6 pt-4 pb-3">
            {/* 顶部操作与指标区 (fixed) */}
            <div className="shrink-0 space-y-3 pb-3 border-b border-white/[0.06]">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div>
                  <h2 className="text-[15px] font-semibold text-white/95">
                    模型资产全景管理
                  </h2>
                  <div className="text-[11px] text-white/40">
                    集中查看所有供应商提供的模型，统一控制启用状态与节点可用性。
                  </div>
                </div>

                <div className="flex items-center gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      providers.forEach((p) => {
                        const updated = (p.models ?? []).map((m) => ({ ...m, enabled: true }));
                        updateProviderModels(p.id, updated);
                      });
                    }}
                    className="h-7 rounded-lg border-white/[0.08] bg-white/[0.03] text-[11px] text-white/80 hover:bg-white/[0.07] px-2.5"
                  >
                    <RotateCcw size={11} className="mr-1" />
                    全部恢复启用
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    onClick={() => {
                      if (providers[0]) {
                        setAddModelTargetProviderId(providers[0].id);
                        setAddModelDialogOpen(true);
                      }
                    }}
                    disabled={providers.length === 0}
                    className="h-7 rounded-lg bg-[#339CFF] px-2.5 text-[11px] font-medium text-white hover:bg-[#2563EB]"
                  >
                    <Plus size={12} className="mr-1" />
                    添加模型
                  </Button>
                </div>
              </div>

              {/* 4 个轻量化指标卡片 */}
              <div className="grid grid-cols-2 md:grid-cols-4 gap-2.5">
                <div className="rounded-xl border border-white/[0.06] bg-white/[0.015] px-3 py-2">
                  <div className="text-[10px] text-white/40">模型总数</div>
                  <div className="mt-0.5 text-[18px] font-bold text-[#339CFF]">
                    {allModelsWithProvider.length}
                  </div>
                  <div className="text-[9px] text-white/30">跨所有服务商</div>
                </div>

                <div className="rounded-xl border border-white/[0.06] bg-white/[0.015] px-3 py-2">
                  <div className="text-[10px] text-white/40">当前已启用</div>
                  <div className="mt-0.5 text-[18px] font-bold text-emerald-400">
                    {totalEnabledModels}
                  </div>
                  <div className="text-[9px] text-white/30">可直接在节点选用</div>
                </div>

                <div className="rounded-xl border border-white/[0.06] bg-white/[0.015] px-3 py-2">
                  <div className="text-[10px] text-white/40">来源服务商</div>
                  <div className="mt-0.5 text-[18px] font-bold text-purple-400">
                    {providers.length}
                  </div>
                  <div className="text-[9px] text-white/30">已配置服务通道</div>
                </div>

                <div className="rounded-xl border border-white/[0.06] bg-white/[0.015] px-3 py-2">
                  <div className="text-[10px] text-white/40">过滤结果</div>
                  <div className="mt-0.5 text-[18px] font-bold text-amber-400">
                    {filteredGlobalModels.length}
                  </div>
                  <div className="text-[9px] text-white/30">当前筛选匹配数</div>
                </div>
              </div>

              {/* 筛选与搜索工具栏 */}
              <div className="flex flex-wrap items-center justify-between gap-2.5">
                <div className="flex items-center gap-2 flex-1 min-w-[240px] max-w-[500px]">
                  <div className="relative flex-1">
                    <Search
                      size={13}
                      className="pointer-events-none absolute left-3 top-2 text-white/35"
                    />
                    <Input
                      value={modelSearch}
                      onChange={(e) => setModelSearch(e.target.value)}
                      placeholder="按模型名称、ID 或服务商筛选..."
                      className="h-7 rounded-lg pl-9 text-[11px] bg-white/[0.03] border-white/[0.08] text-white/90 placeholder:text-white/30"
                    />
                  </div>

                  <Select
                    value={globalProviderFilter}
                    onValueChange={(v) => setGlobalProviderFilter(v)}
                  >
                    <SelectTrigger className="h-7 w-[140px] rounded-lg border-white/[0.08] bg-white/[0.03] text-[11px] text-white/80 shrink-0">
                      <SelectValue placeholder="全部服务商" />
                    </SelectTrigger>
                    <SelectContent className="bg-[#1a1a1a] border-white/10 text-white">
                      <SelectItem value="all" className="text-[11px]">
                        全部服务商 ({providers.length})
                      </SelectItem>
                      {providers.map((p) => (
                        <SelectItem key={p.id} value={p.id} className="text-[11px]">
                          <div className="flex items-center gap-1.5">
                            <ProviderLogo logo={p.logo} name={p.name} size="xs" />
                            <span>{p.name}</span>
                          </div>
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>

                  <Select
                    value={globalScenarioFilter}
                    onValueChange={(v) => setGlobalScenarioFilter(v as ModelScenario | "all")}
                  >
                    <SelectTrigger className="h-7 w-[132px] rounded-lg border-white/[0.08] bg-white/[0.03] text-[11px] text-white/80 shrink-0">
                      <SelectValue placeholder="全部场景" />
                    </SelectTrigger>
                    <SelectContent className="bg-[#1a1a1a] border-white/10 text-white">
                      <SelectItem value="all" className="text-[11px]">
                        全部使用场景
                      </SelectItem>
                      {MODEL_SCENARIO_OPTIONS.map((option) => (
                        <SelectItem key={option.value} value={option.value} className="text-[11px]">
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>

                <div className="flex items-center gap-1.5">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      providers.forEach((p) => {
                        const updated = (p.models ?? []).map((m) => ({ ...m, enabled: true }));
                        updateProviderModels(p.id, updated);
                      });
                    }}
                    className="h-7 rounded-lg border-white/[0.08] bg-white/[0.03] text-[10px] text-white/80 hover:bg-white/[0.07] px-2"
                  >
                    全部启用
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      providers.forEach((p) => {
                        const updated = (p.models ?? []).map((m) => ({ ...m, enabled: false }));
                        updateProviderModels(p.id, updated);
                      });
                    }}
                    className="h-7 rounded-lg border-white/[0.08] bg-white/[0.03] text-[10px] text-white/80 hover:bg-white/[0.07] px-2"
                  >
                    全部禁用
                  </Button>
                </div>
              </div>
            </div>

            {/* 全局模型列表 (表头固定，列表内滚动) */}
            <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden pt-2.5">
              {/* 表头 */}
              <div className="shrink-0 grid grid-cols-[minmax(170px,1.15fr)_140px_minmax(150px,1.5fr)_minmax(170px,1.5fr)_100px] items-center border-b border-white/[0.06] px-3 py-2 text-[10px] font-semibold text-white/40 uppercase tracking-wider">
                <span>模型名称</span>
                <span>来源服务商</span>
                <span>模型描述 / 能力</span>
                <span>使用场景</span>
                <span className="text-right">操作</span>
              </div>

              {/* 真正滚动的表格主体 */}
              <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain divide-y divide-white/[0.04] pr-1">
                {filteredGlobalModels.length === 0 ? (
                  <div className="py-16 text-center text-white/40">
                    <CircleAlert size={22} className="mx-auto mb-2 opacity-30" />
                    <div className="text-[12px]">暂无匹配的模型资产</div>
                  </div>
                ) : (
                  filteredGlobalModels.map((item) => (
                    <div
                      key={`${item.provider.id}-${item.id}`}
                      className="grid grid-cols-[minmax(170px,1.15fr)_140px_minmax(150px,1.5fr)_minmax(170px,1.5fr)_100px] items-center px-3 py-2.5 hover:bg-white/[0.025] transition-colors"
                    >
                      <div className="flex items-center gap-2.5 min-w-0 pr-2">
                        <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-[#339CFF]/10 text-[#339CFF]">
                          <Cpu size={13} />
                        </div>
                        <div className="min-w-0">
                          <div className="truncate text-[12px] font-medium text-white/90">
                            {item.name}
                          </div>
                          <div className="truncate text-[10px] text-white/40 font-mono">
                            {item.id}
                          </div>
                        </div>
                      </div>

                      <div className="min-w-0 pr-2">
                        <span className="inline-flex items-center gap-1.5 rounded-md bg-white/[0.05] px-2 py-0.5 text-[10px] text-white/70">
                          <ProviderLogo logo={item.provider.logo} name={item.provider.name} size="xs" />
                          <span className="truncate">{item.provider.name}</span>
                        </span>
                      </div>

                      <div className="truncate text-[11px] text-white/40 pr-2">
                        {item.description || "通用大模型服务"}
                      </div>

                      <div className="flex min-w-0 flex-wrap gap-1 pr-2">
                        {scenarioLabels(item.scenarios).length > 0 ? (
                          scenarioLabels(item.scenarios).map((label) => (
                            <span
                              key={label}
                              className="rounded bg-[#339CFF]/10 px-1.5 py-0.5 text-[9px] text-[#7CC2FF]"
                            >
                              {label}
                            </span>
                          ))
                        ) : (
                          <span className="text-[10px] text-white/25">未设置场景</span>
                        )}
                      </div>

                      <div className="flex items-center justify-end gap-2">
                        <button
                          type="button"
                          onClick={() =>
                            openScenarioDialog(
                              item.provider.id,
                              item.id,
                              item.name,
                              item.scenarios,
                            )
                          }
                          className="rounded-md p-1 text-white/40 transition-colors hover:bg-[#339CFF]/10 hover:text-[#9DD4FF]"
                          title="设置模型使用场景"
                        >
                          <Settings2 size={13} />
                        </button>
                        <ToggleSwitch
                          checked={item.enabled}
                          onChange={() =>
                            toggleModelEnabled(item.provider.id, item.id)
                          }
                          size="sm"
                        />
                      </div>
                    </div>
                  ))
                )}
              </div>
            </div>
          </main>
        )}
      </div>

      {/* ============================================================= */}
      {/* 弹窗：添加模型 */}
      {/* ============================================================= */}
      <Dialog open={addModelDialogOpen} onOpenChange={setAddModelDialogOpen}>
        <DialogContent
          zIndexClass="z-[11000]"
          className="w-[min(460px,calc(100vw-32px))] rounded-xl border border-white/10 bg-[#16171a] p-0 text-white shadow-2xl"
        >
          <DialogTitle className="sr-only">添加模型资产</DialogTitle>
          <div className="border-b border-white/[0.08] px-5 py-3.5">
            <div className="text-[14px] font-semibold text-white/95">
              添加模型资产
            </div>
            <div className="text-[11px] text-white/40 mt-0.5">
              为所选服务商注册一个可用的大模型 ID。
            </div>
          </div>

          <div className="space-y-3.5 p-5">
            <div className="space-y-1">
              <Label className="text-[11px] text-white/60">目标服务商</Label>
              <Select
                value={addModelTargetProviderId ?? ""}
                onValueChange={(v) => setAddModelTargetProviderId(v)}
              >
                <SelectTrigger className="h-8 rounded-lg border-white/[0.08] bg-white/[0.03] text-[11px] text-white/90">
                  <SelectValue placeholder="选择服务商" />
                </SelectTrigger>
                <SelectContent className="bg-[#1c1d21] border-white/10 text-white">
                  {providers.map((p) => (
                    <SelectItem key={p.id} value={p.id} className="text-[11px]">
                      {p.name} ({p.protocol.toUpperCase()})
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1">
              <Label className="text-[11px] text-white/60">模型 ID (API 调用名) *</Label>
              <Input
                value={newModelForm.id}
                onChange={(e) =>
                  setNewModelForm({ ...newModelForm, id: e.target.value })
                }
                placeholder="如 deepseek-chat, gpt-4o"
                className="h-8 rounded-lg text-[11px] bg-white/[0.03] border-white/[0.08] text-white font-mono placeholder:text-white/30"
              />
            </div>

            <div className="space-y-1">
              <Label className="text-[11px] text-white/60">显示名称 (别名)</Label>
              <Input
                value={newModelForm.name}
                onChange={(e) =>
                  setNewModelForm({ ...newModelForm, name: e.target.value })
                }
                placeholder="如 DeepSeek V3 官方版 (选填)"
                className="h-8 rounded-lg text-[11px] bg-white/[0.03] border-white/[0.08] text-white placeholder:text-white/30"
              />
            </div>

            <div className="space-y-1">
              <Label className="text-[11px] text-white/60">能力或特性描述</Label>
              <Input
                value={newModelForm.description}
                onChange={(e) =>
                  setNewModelForm({ ...newModelForm, description: e.target.value })
                }
                placeholder="如 128K 上下文、支持代码生成"
                className="h-8 rounded-lg text-[11px] bg-white/[0.03] border-white/[0.08] text-white placeholder:text-white/30"
              />
            </div>
          </div>

          <div className="flex justify-end gap-2 border-t border-white/[0.08] px-5 py-3 bg-white/[0.015]">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setAddModelDialogOpen(false)}
              className="h-7 rounded-lg border-white/[0.08] bg-transparent text-white/70 hover:bg-white/[0.05]"
            >
              取消
            </Button>
            <Button
              type="button"
              size="sm"
              disabled={!newModelForm.id.trim() || !addModelTargetProviderId}
              onClick={handleConfirmAddModel}
              className="h-7 rounded-lg bg-[#339CFF] px-3 text-[11px] text-white hover:bg-[#2563EB]"
            >
              确定添加
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {/* ============================================================= */}
      {/* 弹窗：设置模型使用场景 */}
      {/* ============================================================= */}
      <Dialog
        open={Boolean(scenarioDialogTarget)}
        onOpenChange={(open) => {
          if (!open) setScenarioDialogTarget(null);
        }}
      >
        <DialogContent
          zIndexClass="z-[11000]"
          className="w-[min(430px,calc(100vw-32px))] rounded-xl border border-white/10 bg-[#16171a] p-0 text-white shadow-2xl"
        >
          <DialogTitle className="sr-only">设置模型使用场景</DialogTitle>
          <div className="border-b border-white/[0.08] px-5 py-3.5">
            <div className="text-[14px] font-semibold text-white/95">设置模型使用场景</div>
            <div className="mt-0.5 truncate text-[11px] text-white/40">
              {scenarioDialogTarget?.modelName ?? "选择模型"}
            </div>
          </div>

          <div className="space-y-2 p-5">
            <div className="text-[11px] text-white/50">
              选择该模型可被画布节点调用的能力，可同时选择多个场景。
            </div>
            <div className="grid grid-cols-1 gap-2">
              {MODEL_SCENARIO_OPTIONS.map((option) => {
                const checked = draftScenarios.includes(option.value);
                return (
                  <button
                    key={option.value}
                    type="button"
                    role="checkbox"
                    aria-checked={checked}
                    onClick={() => toggleDraftScenario(option.value)}
                    className={cn(
                      "flex items-center justify-between rounded-lg border px-3 py-2.5 text-left text-[12px] transition-colors",
                      checked
                        ? "border-[#339CFF]/45 bg-[#339CFF]/10 text-[#B9E1FF]"
                        : "border-white/[0.08] bg-white/[0.02] text-white/65 hover:border-white/[0.16] hover:bg-white/[0.05]",
                    )}
                  >
                    <span>{option.label}</span>
                    <span
                      className={cn(
                        "flex h-4 w-4 items-center justify-center rounded border",
                        checked
                          ? "border-[#339CFF] bg-[#339CFF] text-white"
                          : "border-white/20 text-transparent",
                      )}
                    >
                      <Check size={11} strokeWidth={2.5} />
                    </span>
                  </button>
                );
              })}
            </div>
          </div>

          <div className="flex justify-end gap-2 border-t border-white/[0.08] bg-white/[0.015] px-5 py-3">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setScenarioDialogTarget(null)}
              className="h-7 rounded-lg border-white/[0.08] bg-transparent text-[11px] text-white/70 hover:bg-white/[0.05]"
            >
              取消
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={handleSaveScenarios}
              className="h-7 rounded-lg bg-[#339CFF] px-3 text-[11px] text-white hover:bg-[#2563EB]"
            >
              保存
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {/* ============================================================= */}
      {/* 弹窗：删除服务商确认 */}
      {/* ============================================================= */}
      <Dialog
        open={Boolean(deleteConfirmProvider)}
        onOpenChange={(open) => {
          if (!open) setDeleteConfirmProvider(null);
        }}
      >
        <DialogContent
          zIndexClass="z-[11000]"
          className="w-[min(380px,calc(100vw-32px))] rounded-xl border border-red-500/20 bg-[#171415] p-0 text-white shadow-2xl"
        >
          <DialogTitle className="sr-only">确认删除服务商</DialogTitle>
          <div className="p-5 text-center space-y-2.5">
            <div className="mx-auto flex h-10 w-10 items-center justify-center rounded-xl bg-red-500/15 text-red-400">
              <Trash2 size={18} />
            </div>
            <div className="text-[14px] font-semibold text-white/95">
              确认删除服务商「{deleteConfirmProvider?.name}」？
            </div>
            <div className="text-[11px] text-white/50 leading-relaxed max-w-xs mx-auto">
              删除后，该服务商下的所有模型配置将被移除。
            </div>
          </div>

          <div className="flex justify-end gap-2 border-t border-white/[0.08] px-5 py-3 bg-white/[0.015]">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setDeleteConfirmProvider(null)}
              className="h-7 rounded-lg border-white/[0.08] bg-transparent text-white/70 hover:bg-white/[0.05]"
            >
              取消
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={() => {
                if (deleteConfirmProvider) {
                  removeProvider(deleteConfirmProvider.id);
                  setDeleteConfirmProvider(null);
                }
              }}
              className="h-7 rounded-lg bg-red-600 px-3 text-[11px] text-white hover:bg-red-700"
            >
              确认删除
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {/* ============================================================= */}
      {/* 弹窗：添加 AI 服务商 */}
      {/* ============================================================= */}
      {/* ============================================================= */}
      {/* 弹窗：添加 AI 供应商 */}
      {/* ============================================================= */}
      <Dialog open={addProviderDialogOpen} onOpenChange={setAddProviderDialogOpen}>
        <DialogContent
          zIndexClass="z-[11000]"
          className="w-[min(430px,calc(100vw-32px))] rounded-xl border border-white/10 bg-[#16171a] p-0 text-white shadow-2xl overflow-hidden flex flex-col"
        >
          <DialogTitle className="sr-only">添加供应商</DialogTitle>

          {/* Header */}
          <div className="shrink-0 border-b border-white/[0.08] px-5 py-3 flex items-center justify-between">
            <div className="text-[14px] font-semibold text-white/95">
              添加供应商
            </div>
          </div>

          {/* Form Content */}
          <div className="p-5 space-y-4">
            {/* 1. 居中 Logo 预览与上传 */}
            <div className="flex flex-col items-center justify-center pt-1 pb-1">
              <div className="relative group">
                <div
                  onClick={() => fileInputRef.current?.click()}
                  title={providerForm.logo ? "点击更换 Logo" : "点击上传 Logo (可选)"}
                  className={`flex h-20 w-20 cursor-pointer items-center justify-center rounded-xl border-2 border-dashed transition-all overflow-hidden ${
                    providerForm.logo
                      ? "border-white/30 bg-black/40"
                      : "border-white/15 bg-white/[0.02] hover:border-[#339CFF]/60 hover:bg-[#339CFF]/5 text-white/35 hover:text-[#339CFF]"
                  }`}
                >
                  {providerForm.logo ? (
                    <img
                      src={providerForm.logo}
                      alt="Logo 预览"
                      className="h-full w-full object-cover"
                    />
                  ) : (
                    <div className="flex flex-col items-center justify-center gap-1">
                      <ImagePlus size={22} strokeWidth={1.8} />
                      <span className="text-[10.5px] font-medium tracking-wide">Logo</span>
                    </div>
                  )}
                </div>

                {providerForm.logo && (
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      setProviderForm((prev) => ({ ...prev, logo: "" }));
                    }}
                    title="清除 Logo"
                    className="absolute -right-1.5 -top-1.5 flex h-4.5 w-4.5 items-center justify-center rounded-full bg-red-500 text-white shadow hover:bg-red-600 transition-colors"
                  >
                    <X size={10} strokeWidth={2.5} />
                  </button>
                )}
              </div>

              <input
                ref={fileInputRef}
                type="file"
                accept="image/*"
                className="hidden"
                onChange={(e) => {
                  const file = e.target.files?.[0];
                  if (file) {
                    processImageFile(file, (dataUrl) => {
                      setProviderForm((prev) => ({ ...prev, logo: dataUrl }));
                    });
                  }
                  e.target.value = "";
                }}
              />
            </div>

            {/* 2. 两列并排：供应商名称 + 协议下拉框 */}
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1.5">
                <Label className="text-[11.5px] font-medium text-white/80">
                  供应商名称 <span className="text-red-400">*</span>
                </Label>
                <Input
                  value={providerForm.name}
                  onChange={(e) =>
                    setProviderForm({ ...providerForm, name: e.target.value })
                  }
                  placeholder="如：DeepSeek 官方"
                  className="h-9 rounded-lg text-[12px] bg-white/[0.03] border-white/[0.08] text-white placeholder:text-white/30 focus-visible:ring-1 focus-visible:ring-[#339CFF]"
                />
              </div>

              <div className="space-y-1.5">
                <Label className="text-[11.5px] font-medium text-white/80">
                  协议下拉框 <span className="text-red-400">*</span>
                </Label>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <button
                      type="button"
                      className="flex h-9 w-full items-center justify-between gap-2 rounded-lg border border-white/[0.08] bg-white/[0.03] px-3 text-[12px] text-white/90 hover:bg-white/[0.06] transition-colors focus:outline-none focus:ring-1 focus:ring-[#339CFF]"
                    >
                      <span>
                        {PROTOCOLS.find((p) => p.value === providerForm.protocol)?.label ||
                          providerForm.protocol}
                      </span>
                      <ChevronDown size={14} className="text-white/40 shrink-0" />
                    </button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent
                    align="start"
                    sideOffset={4}
                    className="!z-[20000] min-w-[160px] rounded-xl border border-white/10 !bg-[#1c1d21] p-1 text-white shadow-2xl"
                  >
                    {PROTOCOLS.map((p) => {
                      const active = providerForm.protocol === p.value;
                      return (
                        <DropdownMenuItem
                          key={p.value}
                          onClick={() =>
                            setProviderForm({ ...providerForm, protocol: p.value })
                          }
                          className={cn(
                            "flex items-center justify-between rounded-lg px-2.5 py-1.5 text-[11.5px] cursor-pointer outline-none transition-colors",
                            active
                              ? "bg-[#339CFF]/15 text-[#339CFF] font-medium"
                              : "text-white/80 hover:bg-white/[0.08] hover:text-white focus:bg-white/[0.08] focus:text-white data-[highlighted]:bg-white/[0.08] data-[highlighted]:text-white",
                          )}
                        >
                          <span>{p.label}</span>
                          {active && <Check size={12} className="text-[#339CFF] shrink-0" />}
                        </DropdownMenuItem>
                      );
                    })}
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </div>

            {/* 3. API 代理地址 (整行) */}
            <div className="space-y-1.5">
              <Label className="text-[11.5px] font-medium text-white/80">
                API 代理地址 <span className="text-red-400">*</span>
              </Label>
              <Input
                value={providerForm.baseUrl}
                onChange={(e) =>
                  setProviderForm({ ...providerForm, baseUrl: e.target.value })
                }
                placeholder="如：https://api.deepseek.com/v1"
                className="h-9 rounded-lg text-[12px] bg-white/[0.03] border-white/[0.08] text-white font-mono placeholder:text-white/30 focus-visible:ring-1 focus-visible:ring-[#339CFF]"
              />
            </div>
          </div>

          {/* Footer */}
          <div className="shrink-0 flex justify-end gap-2.5 border-t border-white/[0.08] px-5 py-3 bg-white/[0.015]">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setAddProviderDialogOpen(false)}
              className="h-8 rounded-lg border-white/[0.08] bg-transparent px-3.5 text-[12px] text-white/70 hover:bg-white/[0.05]"
            >
              取消
            </Button>
            <Button
              type="button"
              size="sm"
              disabled={!providerForm.name.trim() || !providerForm.baseUrl.trim()}
              onClick={handleConfirmAddProvider}
              className="h-8 rounded-lg bg-[#339CFF] px-4 text-[12px] text-white hover:bg-[#2563EB] disabled:opacity-40"
            >
              确定添加
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {/* ============================================================= */}
      {/* 弹窗：更换供应商 Logo */}
      {/* ============================================================= */}
      <Dialog open={changeLogoDialogOpen} onOpenChange={setChangeLogoDialogOpen}>
        <DialogContent
          zIndexClass="z-[11000]"
          className="w-[min(360px,calc(100vw-32px))] rounded-xl border border-white/10 bg-[#16171a] p-0 text-white shadow-2xl overflow-hidden"
        >
          <DialogTitle className="sr-only">更换服务商 Logo</DialogTitle>
          <div className="border-b border-white/[0.08] px-4 py-2.5 flex items-center justify-between">
            <div className="text-[13px] font-semibold text-white/95">
              更换「{selectedProvider?.name}」Logo
            </div>
          </div>

          <div className="p-4 flex items-center gap-3">
            <div
              onClick={() => changeLogoFileInputRef.current?.click()}
              title="点击上传本地图片"
              className="flex h-11 w-11 cursor-pointer items-center justify-center rounded-xl border border-dashed border-white/20 bg-white/[0.03] transition-all hover:border-[#339CFF]/60 hover:bg-[#339CFF]/10 overflow-hidden shrink-0"
            >
              <ProviderLogo
                logo={selectedProvider?.logo}
                name={selectedProvider?.name}
                size="lg"
              />
            </div>

            <div className="min-w-0 flex-1 space-y-1">
              <div className="text-[11px] text-white/80">点击左侧图标上传本地图片</div>
              <div className="text-[10px] text-white/35">支持 PNG, JPG, SVG, WebP 格式</div>
              {selectedProvider?.logo && (
                <button
                  type="button"
                  onClick={() => {
                    if (selectedProvider) {
                      updateProvider(selectedProvider.id, { logo: undefined });
                      setChangeLogoDialogOpen(false);
                    }
                  }}
                  className="text-[10.5px] text-red-400 hover:underline transition-colors block pt-0.5"
                >
                  恢复系统默认 Logo
                </button>
              )}
            </div>

            <input
              ref={changeLogoFileInputRef}
              type="file"
              accept="image/*"
              className="hidden"
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (file && selectedProvider) {
                  processImageFile(file, (dataUrl) => {
                    updateProvider(selectedProvider.id, { logo: dataUrl });
                    setChangeLogoDialogOpen(false);
                  });
                }
                e.target.value = "";
              }}
            />
          </div>

          <div className="flex justify-end border-t border-white/[0.08] px-4 py-2.5 bg-white/[0.015]">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setChangeLogoDialogOpen(false)}
              className="h-7 rounded-lg border-white/[0.08] bg-transparent text-[11px] text-white/70 hover:bg-white/[0.05]"
            >
              完成
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
