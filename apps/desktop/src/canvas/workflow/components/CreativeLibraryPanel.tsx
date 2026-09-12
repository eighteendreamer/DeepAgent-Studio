import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  X, Plus, Search, Trash2, Star, Download, Upload,
  ChevronDown, ChevronRight, Check, FolderOpen, Image as ImageIcon,
  Folder,
} from "lucide-react";
import { CATEGORY_TREE, type CategoryNode } from "../utils/categoryTree";

// ─── Types ───────────────────────────────────────────────────────────────────

interface CreativeItem {
  id: string;
  name: string;
  category: string;
  prompt?: string;
  imageUrl?: string;
  isFavorite?: boolean;
  createdAt: number | string;
  order?: number;
}

type SortKey = "time" | "name" | "manual";
type FilterTab = "all" | "favorite";

// ─── Constants ───────────────────────────────────────────────────────────────

const STORAGE_KEY = "canvas-creative-library";
const GRID_PAGE = 24;


// ─── Storage ─────────────────────────────────────────────────────────────────

function loadItems(): CreativeItem[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
}

function saveItems(items: CreativeItem[]) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(items));
}

function normalizeTimestamp(t: number | string): number {
  if (typeof t === "number") return t;
  return new Date(t).getTime() || 0;
}

// ─── Component ───────────────────────────────────────────────────────────────

interface Props {
  onClose: () => void;
  onUse?: (item: CreativeItem) => void;
}

export function CreativeLibraryPanel({ onClose, onUse }: Props) {
  const [items, setItems] = useState<CreativeItem[]>(loadItems);
  const [search, setSearch] = useState("");
  const [sortKey, setSortKey] = useState<SortKey>("time");
  const [filterTab, setFilterTab] = useState<FilterTab>("all");
  const [selectedCategory, setSelectedCategory] = useState("all");
  const [multiSelect, setMultiSelect] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [showAdd, setShowAdd] = useState(false);
  const [visibleCount, setVisibleCount] = useState(GRID_PAGE);
  const [sortOpen, setSortOpen] = useState(false);
  const [expandedCats, setExpandedCats] = useState<Set<string>>(new Set(["environment_design", "tool"]));
  const sentinelRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  // Persist on change
  useEffect(() => { saveItems(items); }, [items]);

  // Escape to close
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // Infinite scroll
  useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => { if (entries[0].isIntersecting) setVisibleCount((c) => c + GRID_PAGE); },
      { rootMargin: "200px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, []);

  // Filtered + sorted
  const filtered = useMemo(() => {
    let result = items.filter((item) => {
      const matchSearch = !search || item.name.toLowerCase().includes(search.toLowerCase()) || (item.prompt ?? "").toLowerCase().includes(search.toLowerCase());
      const matchCategory = selectedCategory === "all" || item.category === selectedCategory;
      const matchFavorite = filterTab === "all" || item.isFavorite;
      return matchSearch && matchCategory && matchFavorite;
    });

    if (sortKey === "time") {
      result.sort((a, b) => normalizeTimestamp(b.createdAt) - normalizeTimestamp(a.createdAt));
    } else if (sortKey === "name") {
      result.sort((a, b) => a.name.localeCompare(b.name, "zh"));
    }
    // manual: keep order field or insertion order
    return result;
  }, [items, search, selectedCategory, filterTab, sortKey]);

  const visible = filtered.slice(0, visibleCount);

  // Category counts
  const categoryCounts = useMemo(() => {
    const counts: Record<string, number> = { all: items.length };
    for (const item of items) {
      counts[item.category] = (counts[item.category] || 0) + 1;
    }
    return counts;
  }, [items]);

  // Actions
  const toggleFavorite = useCallback((id: string) => {
    setItems((prev) => prev.map((it) => it.id === id ? { ...it, isFavorite: !it.isFavorite } : it));
  }, []);

  const deleteItem = useCallback((id: string) => {
    setItems((prev) => prev.filter((it) => it.id !== id));
    setSelectedIds((prev) => { const n = new Set(prev); n.delete(id); return n; });
  }, []);

  const deleteSelected = useCallback(() => {
    if (!selectedIds.size) return;
    setItems((prev) => prev.filter((it) => !selectedIds.has(it.id)));
    setSelectedIds(new Set());
    setMultiSelect(false);
  }, [selectedIds]);

  const toggleSelect = useCallback((id: string) => {
    setSelectedIds((prev) => {
      const n = new Set(prev);
      if (n.has(id)) n.delete(id); else n.add(id);
      return n;
    });
  }, []);

  const selectAll = useCallback(() => {
    setSelectedIds(new Set(filtered.map((it) => it.id)));
  }, [filtered]);

  const clearSelection = useCallback(() => {
    setSelectedIds(new Set());
  }, []);

  const toggleExpanded = (key: string) => {
    setExpandedCats((prev) => {
      const n = new Set(prev);
      if (n.has(key)) n.delete(key); else n.add(key);
      return n;
    });
  };

  const renderSidebarTree = (nodes: CategoryNode[], depth: number): React.ReactNode =>
    nodes.map((node) => {
      const hasChildren = node.children && node.children.length > 0;
      const isExpanded = expandedCats.has(node.key);
      const isActive = selectedCategory === node.key;
      const count = categoryCounts[node.key] || 0;
      const indent = depth * 12;

      return (
        <div key={node.key}>
          <button
            onClick={() => {
              setSelectedCategory(node.key);
              setVisibleCount(GRID_PAGE);
              if (hasChildren) toggleExpanded(node.key);
            }}
            className="flex w-full items-center justify-between rounded-lg px-2 py-1.5 text-xs transition-colors"
            style={{
              paddingLeft: 8 + indent,
              background: isActive ? "rgba(139,92,246,0.15)" : "transparent",
              color: isActive ? "#8b5cf6" : "rgba(255,255,255,0.7)",
            }}
          >
            <span className="flex items-center gap-1.5 min-w-0">
              {hasChildren ? (
                <ChevronRight className="h-3 w-3 shrink-0 transition-transform" style={{ transform: isExpanded ? "rotate(90deg)" : "none" }} />
              ) : (
                <span className="w-3" />
              )}
              <node.icon className="h-3.5 w-3.5 shrink-0" />
              <span className="truncate">{node.label}</span>
            </span>
            <span className="text-[10px] shrink-0 ml-1" style={{ color: "rgba(255,255,255,0.35)" }}>{count}</span>
          </button>
          {hasChildren && isExpanded && renderSidebarTree(node.children!, depth + 1)}
        </div>
      );
    });

  const handleUse = useCallback((item: CreativeItem) => {
    onUse?.(item);
    onClose();
  }, [onUse, onClose]);

  const handleAddItem = useCallback((item: Omit<CreativeItem, "id" | "createdAt">) => {
    const newItem: CreativeItem = {
      ...item,
      id: `creative-${Date.now()}`,
      createdAt: Date.now(),
    };
    setItems((prev) => [newItem, ...prev]);
    setShowAdd(false);
  }, []);

  const handleExport = useCallback(() => {
    const data = JSON.stringify(items, null, 2);
    const blob = new Blob([data], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `creative_library_${new Date().toISOString().slice(0, 10)}.json`;
    a.click();
    URL.revokeObjectURL(url);
  }, [items]);

  const handleImport = useCallback(() => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".json";
    input.onchange = async () => {
      const file = input.files?.[0];
      if (!file) return;
      try {
        const text = await file.text();
        const imported: CreativeItem[] = JSON.parse(text);
        if (!Array.isArray(imported)) return;
        const existingIds = new Set(items.map((it) => it.id));
        const newItems = imported
          .filter((it) => !existingIds.has(it.id))
          .map((it) => ({ ...it, id: it.id || `creative-${Date.now()}-${Math.random().toString(36).slice(2, 8)}` }));
        setItems((prev) => [...newItems, ...prev]);
      } catch { /* invalid file */ }
    };
    input.click();
  }, [items]);

  // ── Render ──────────────────────────────────────────────────────────────

  return createPortal(
    <div
      className="fixed inset-0 flex flex-col"
      style={{ zIndex: 10000, background: "rgba(0,0,0,0.85)" }}
      onMouseDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-3" style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}>
        <div className="flex items-center gap-2">
          <div className="h-3 w-3 rounded-sm" style={{ background: "#8b5cf6" }} />
          <span className="text-base font-semibold" style={{ color: "rgba(255,255,255,0.9)" }}>创意库</span>
        </div>
        <button onClick={onClose} className="flex h-8 w-8 items-center justify-center rounded-lg transition-colors hover:bg-white/10">
          <X className="h-4 w-4" style={{ color: "rgba(255,255,255,0.7)" }} />
        </button>
      </div>

      {/* Toolbar */}
      <div className="flex items-center justify-between px-4 py-2" style={{ borderBottom: "1px solid rgba(255,255,255,0.06)", background: "rgba(255,255,255,0.02)" }}>
        <div className="flex items-center gap-2">
          {/* Search */}
          <div className="relative">
            <Search className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2" style={{ color: "rgba(255,255,255,0.4)" }} />
            <input
              ref={searchRef}
              value={search}
              onChange={(e) => { setSearch(e.target.value); setVisibleCount(GRID_PAGE); }}
              placeholder="搜索创意..."
              className="rounded-lg py-1.5 pl-8 pr-3 text-xs outline-none"
              style={{ width: 200, background: "rgba(255,255,255,0.05)", border: "1px solid rgba(255,255,255,0.08)", color: "rgba(255,255,255,0.85)" }}
            />
          </div>

          {/* Sort */}
          <div className="relative">
            <button
              onClick={() => setSortOpen(!sortOpen)}
              className="flex items-center gap-1 rounded-lg px-2.5 py-1.5 text-xs transition-colors hover:bg-white/10"
              style={{ color: "rgba(255,255,255,0.7)", border: "1px solid rgba(255,255,255,0.08)" }}
            >
              {sortKey === "time" ? "时间" : sortKey === "name" ? "名称" : "手动"}
              <ChevronDown className="h-3 w-3" />
            </button>
            {sortOpen && (
              <div className="absolute top-full left-0 mt-1 rounded-lg py-1" style={{ background: "rgba(30,30,35,0.98)", border: "1px solid rgba(255,255,255,0.1)", zIndex: 10 }}>
                {(["time", "name", "manual"] as SortKey[]).map((k) => (
                  <button
                    key={k}
                    onClick={() => { setSortKey(k); setSortOpen(false); }}
                    className="block w-full px-3 py-1.5 text-left text-xs transition-colors hover:bg-white/10"
                    style={{ color: sortKey === k ? "#8b5cf6" : "rgba(255,255,255,0.7)" }}
                  >
                    {k === "time" ? "时间" : k === "name" ? "名称" : "手动"}
                  </button>
                ))}
              </div>
            )}
          </div>

          {/* Multi-select */}
          <button
            onClick={() => { setMultiSelect(!multiSelect); setSelectedIds(new Set()); }}
            className="flex items-center gap-1 rounded-lg px-2.5 py-1.5 text-xs transition-colors hover:bg-white/10"
            style={{ color: multiSelect ? "#8b5cf6" : "rgba(255,255,255,0.7)", border: "1px solid rgba(255,255,255,0.08)" }}
          >
            <Check className="h-3 w-3" />
            多选
          </button>

          {/* Filter tabs */}
          <div className="flex items-center gap-1 ml-2">
            {([["all", "全部"], ["favorite", "收藏"]] as [FilterTab, string][]).map(([key, label]) => (
              <button
                key={key}
                onClick={() => setFilterTab(key)}
                className="flex items-center gap-1 rounded-lg px-2.5 py-1.5 text-xs font-medium transition-colors"
                style={{
                  background: filterTab === key ? "rgba(139,92,246,0.2)" : "transparent",
                  color: filterTab === key ? "#8b5cf6" : "rgba(255,255,255,0.5)",
                }}
              >
                {key === "favorite" && <Star className="h-3.5 w-3.5" style={{ fill: filterTab === "favorite" ? "#fbbf24" : "none", color: filterTab === "favorite" ? "#fbbf24" : "currentColor" }} />}
                {label}
              </button>
            ))}
          </div>
        </div>

        <div className="flex items-center gap-2">
          {multiSelect && selectedIds.size > 0 && (
            <div className="flex items-center gap-2 mr-2">
              <span className="text-xs" style={{ color: "rgba(255,255,255,0.5)" }}>已选 {selectedIds.size} 项</span>
              <button onClick={selectAll} className="text-xs hover:text-white/80" style={{ color: "rgba(255,255,255,0.5)" }}>全选</button>
              <button onClick={clearSelection} className="text-xs hover:text-white/80" style={{ color: "rgba(255,255,255,0.5)" }}>取消</button>
              <button onClick={deleteSelected} className="flex items-center gap-1 rounded-lg px-2 py-1 text-xs hover:bg-red-500/20" style={{ color: "#ef4444" }}>
                <Trash2 className="h-3 w-3" /> 删除
              </button>
            </div>
          )}
          <button onClick={handleImport} className="flex items-center gap-1 rounded-lg px-2.5 py-1.5 text-xs transition-colors hover:bg-white/10" style={{ color: "rgba(255,255,255,0.7)" }}>
            <Upload className="h-3.5 w-3.5" /> 导入
          </button>
          <button onClick={handleExport} className="flex items-center gap-1 rounded-lg px-2.5 py-1.5 text-xs transition-colors hover:bg-white/10" style={{ color: "rgba(255,255,255,0.7)" }}>
            <Download className="h-3.5 w-3.5" /> 导出
          </button>
          <button
            onClick={() => setShowAdd(true)}
            className="flex items-center gap-1 rounded-lg px-3 py-1.5 text-xs font-medium transition-colors"
            style={{ background: "rgba(139,92,246,0.2)", color: "#8b5cf6" }}
          >
            <Plus className="h-3.5 w-3.5" /> 新增
          </button>
        </div>
      </div>

      {/* Body: sidebar + grid */}
      <div className="flex flex-1 overflow-hidden">
        {/* Sidebar */}
        <div className="creative-sidebar-scroll flex flex-col overflow-y-auto py-3" style={{ width: 180, borderRight: "1px solid rgba(255,255,255,0.06)" }}>
          <div className="px-3 mb-2 text-[10px] font-medium uppercase tracking-wider" style={{ color: "rgba(255,255,255,0.35)" }}>分类</div>
          <button
            onClick={() => { setSelectedCategory("all"); setVisibleCount(GRID_PAGE); }}
            className="mx-2 flex items-center justify-between rounded-lg px-2 py-1.5 text-xs transition-colors"
            style={{
              background: selectedCategory === "all" ? "rgba(139,92,246,0.15)" : "transparent",
              color: selectedCategory === "all" ? "#8b5cf6" : "rgba(255,255,255,0.7)",
            }}
          >
            <span className="flex items-center gap-1.5"><Folder className="h-3.5 w-3.5" /><span>全部</span></span>
            <span className="text-[10px]" style={{ color: "rgba(255,255,255,0.35)" }}>{items.length}</span>
          </button>
          {renderSidebarTree(CATEGORY_TREE, 0)}
        </div>

        {/* Main grid */}
        <div className="flex-1 overflow-y-auto p-4">
          {visible.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-20">
              <FolderOpen className="mb-3 h-12 w-12" style={{ color: "rgba(255,255,255,0.15)" }} />
              <div className="text-sm font-medium" style={{ color: "rgba(255,255,255,0.4)" }}>
                {items.length === 0 ? "创意库是空的" : "未找到创意"}
              </div>
              <div className="mt-1 text-xs" style={{ color: "rgba(255,255,255,0.3)" }}>
                {items.length === 0 ? "点击「新增」来添加您的第一个灵感" : "请尝试其他关键词或筛选条件"}
              </div>
            </div>
          ) : (
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
              {visible.map((item) => (
                <div
                  key={item.id}
                  className="group relative rounded-xl overflow-hidden transition-all hover:ring-1 hover:ring-white/10"
                  style={{ background: "rgba(255,255,255,0.03)", border: "1px solid rgba(255,255,255,0.06)" }}
                >
                  {/* Thumbnail */}
                  <div className="relative" style={{ aspectRatio: "4/3", background: "rgba(255,255,255,0.02)" }}>
                    {item.imageUrl ? (
                      <img
                        src={item.imageUrl}
                        alt={item.name}
                        loading="lazy"
                        decoding="async"
                        className="h-full w-full object-cover transition-transform duration-200 group-hover:scale-105"
                        style={{ pointerEvents: "none" }}
                      />
                    ) : (
                      <div className="flex h-full w-full items-center justify-center">
                        <ImageIcon className="h-8 w-8" style={{ color: "rgba(255,255,255,0.1)" }} />
                      </div>
                    )}

                    {/* Hover overlay */}
                    <div className="absolute inset-0 flex items-center justify-center gap-2 opacity-0 transition-opacity group-hover:opacity-100" style={{ background: "rgba(0,0,0,0.5)" }}>
                      <button
                        onClick={() => toggleFavorite(item.id)}
                        className="flex h-8 w-8 items-center justify-center rounded-full transition-colors hover:bg-white/20"
                        style={{ background: "rgba(255,255,255,0.15)" }}
                      >
                        <Star className="h-4 w-4" style={{ color: item.isFavorite ? "#fbbf24" : "white", fill: item.isFavorite ? "#fbbf24" : "none" }} />
                      </button>
                      <button
                        onClick={() => handleUse(item)}
                        className="flex h-8 w-8 items-center justify-center rounded-full transition-colors hover:bg-white/20"
                        style={{ background: "rgba(139,92,246,0.6)" }}
                      >
                        <Check className="h-4 w-4" style={{ color: "white" }} />
                      </button>
                      <button
                        onClick={() => deleteItem(item.id)}
                        className="flex h-8 w-8 items-center justify-center rounded-full transition-colors hover:bg-red-500/40"
                        style={{ background: "rgba(255,255,255,0.15)" }}
                      >
                        <Trash2 className="h-4 w-4" style={{ color: "white" }} />
                      </button>
                    </div>

                    {/* Multi-select checkbox */}
                    {multiSelect && (
                      <button
                        onClick={() => toggleSelect(item.id)}
                        className="absolute top-2 left-2 flex h-5 w-5 items-center justify-center rounded"
                        style={{ background: selectedIds.has(item.id) ? "#8b5cf6" : "rgba(0,0,0,0.5)", border: "1px solid rgba(255,255,255,0.3)" }}
                      >
                        {selectedIds.has(item.id) && <Check className="h-3 w-3" style={{ color: "white" }} />}
                      </button>
                    )}

                    {/* Favorite badge */}
                    {item.isFavorite && !multiSelect && (
                      <div className="absolute top-2 right-2">
                        <Star className="h-4 w-4" style={{ color: "#fbbf24", fill: "#fbbf24" }} />
                      </div>
                    )}
                  </div>

                  {/* Info */}
                  <div className="p-2.5">
                    <div className="truncate text-xs font-medium" style={{ color: "rgba(255,255,255,0.85)" }}>{item.name}</div>
                    <div className="mt-0.5 flex items-center justify-between">
                      <span className="text-[10px]" style={{ color: "rgba(139,92,246,0.7)" }}>{item.category}</span>
                      <button
                        onClick={() => handleUse(item)}
                        className="rounded px-2 py-0.5 text-[10px] font-medium transition-colors hover:bg-white/10"
                        style={{ background: "rgba(139,92,246,0.15)", color: "#8b5cf6" }}
                      >
                        使用
                      </button>
                    </div>
                  </div>
                </div>
              ))}
            </div>
          )}
          <div ref={sentinelRef} className="h-1" />
        </div>
      </div>

      {/* Add modal */}
      {showAdd && <AddCreativeModal onClose={() => setShowAdd(false)} onSave={handleAddItem} />}
    </div>,
    document.body,
  );
}

// ─── Add Modal ───────────────────────────────────────────────────────────────

function AddCreativeModal({ onClose, onSave }: { onClose: () => void; onSave: (item: Omit<CreativeItem, "id" | "createdAt">) => void }) {
  const [name, setName] = useState("");
  const [category, setCategory] = useState("其他");
  const [prompt, setPrompt] = useState("");
  const [imageUrl, setImageUrl] = useState("");
  const [dragOver, setDragOver] = useState(false);

  const handleFile = (file: File) => {
    if (!file.type.startsWith("image/")) return;
    const reader = new FileReader();
    reader.onload = () => setImageUrl(reader.result as string);
    reader.readAsDataURL(file);
  };

  const handleSave = () => {
    if (!name.trim()) return;
    onSave({ name: name.trim(), category, prompt: prompt.trim() || undefined, imageUrl: imageUrl || undefined });
  };

  return createPortal(
    <div className="fixed inset-0 flex items-center justify-center" style={{ zIndex: 10001, background: "rgba(0,0,0,0.7)" }} onMouseDown={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()} onContextMenu={(e) => e.stopPropagation()}>
      <div className="flex flex-col rounded-2xl" style={{ width: 560, maxHeight: "85vh", background: "rgba(30,30,35,0.98)", border: "1px solid rgba(255,255,255,0.08)" }}>
        <div className="flex items-center justify-between px-5 py-3" style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}>
          <span className="text-sm font-medium" style={{ color: "rgba(255,255,255,0.85)" }}>新增创意</span>
          <button onClick={onClose} className="flex h-7 w-7 items-center justify-center rounded-lg hover:bg-white/10">
            <X className="h-4 w-4" style={{ color: "rgba(255,255,255,0.6)" }} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-5">
          {/* Image upload */}
          <div
            className="mb-4 flex items-center justify-center rounded-xl"
            style={{ height: 160, border: `2px dashed ${dragOver ? "#8b5cf6" : "rgba(255,255,255,0.12)"}`, background: "rgba(255,255,255,0.02)" }}
            onDragOver={(e) => { e.preventDefault(); setDragOver(true); }}
            onDragLeave={() => setDragOver(false)}
            onDrop={(e) => { e.preventDefault(); setDragOver(false); handleFile(e.dataTransfer.files[0]); }}
            onClick={() => { const i = document.createElement("input"); i.type = "file"; i.accept = "image/*"; i.onchange = () => i.files?.[0] && handleFile(i.files[0]); i.click(); }}
          >
            {imageUrl ? (
              <img src={imageUrl} alt="preview" className="h-full w-full rounded-lg object-cover" />
            ) : (
              <div className="text-center">
                <ImageIcon className="mx-auto mb-1 h-8 w-8" style={{ color: "rgba(255,255,255,0.2)" }} />
                <div className="text-xs" style={{ color: "rgba(255,255,255,0.4)" }}>拖放或点击上传图片</div>
              </div>
            )}
          </div>

          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="名称" className="mb-3 w-full rounded-lg px-3 py-2 text-xs outline-none" style={{ background: "rgba(255,255,255,0.05)", border: "1px solid rgba(255,255,255,0.08)", color: "rgba(255,255,255,0.85)" }} />

          <div className="mb-3">
            <div className="mb-2 text-[10px] font-medium uppercase tracking-wider" style={{ color: "rgba(255,255,255,0.4)" }}>选择分类</div>
            <div className="max-h-48 overflow-y-auto rounded-lg" style={{ background: "rgba(255,255,255,0.02)", border: "1px solid rgba(255,255,255,0.08)" }}>
              {(() => {
                const renderTree = (nodes: CategoryNode[], depth: number): React.ReactNode =>
                  nodes.map((node) => {
                    const hasChildren = node.children && node.children.length > 0;
                    const isActive = category === node.key;
                    const indent = depth * 12;
                    return (
                      <div key={node.key}>
                        <button
                          type="button"
                          onClick={() => setCategory(node.key)}
                          className="flex w-full items-center gap-1.5 px-2 py-1.5 text-xs transition-colors hover:bg-white/5"
                          style={{
                            paddingLeft: 8 + indent,
                            background: isActive ? "rgba(139,92,246,0.15)" : "transparent",
                            color: isActive ? "#8b5cf6" : "rgba(255,255,255,0.7)",
                          }}
                        >
                          <node.icon className="h-3.5 w-3.5 shrink-0" />
                          <span className="truncate">{node.label}</span>
                        </button>
                        {hasChildren && renderTree(node.children!, depth + 1)}
                      </div>
                    );
                  });
                return renderTree(CATEGORY_TREE, 0);
              })()}
            </div>
          </div>

          <textarea value={prompt} onChange={(e) => setPrompt(e.target.value)} placeholder="提示词（可选）" rows={4} className="w-full resize-none rounded-lg px-3 py-2 text-xs outline-none" style={{ background: "rgba(255,255,255,0.05)", border: "1px solid rgba(255,255,255,0.08)", color: "rgba(255,255,255,0.85)" }} />
        </div>

        <div className="flex justify-end gap-2 px-5 py-3" style={{ borderTop: "1px solid rgba(255,255,255,0.08)" }}>
          <button onClick={onClose} className="rounded-lg px-4 py-2 text-xs transition-colors hover:bg-white/10" style={{ color: "rgba(255,255,255,0.6)" }}>取消</button>
          <button onClick={handleSave} disabled={!name.trim()} className="rounded-lg px-4 py-2 text-xs font-medium disabled:opacity-40" style={{ background: "#8b5cf6", color: "white" }}>保存</button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
