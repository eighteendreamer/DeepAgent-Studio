import { HoverInfo } from "../ui/HoverInfo";
import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Box, ChevronRight, Code2, FileText, Maximize, Network, RotateCw, Search, ZoomIn, ZoomOut } from "lucide-react";
import { Button } from "../shadcn/button";
import {
  projectMapGraph,
  projectMapNeighbors,
  projectMapOverview,
  projectMapRefreshDeep,
  projectMapSearch,
} from "../../api";
import type {
  ProjectMapHit,
  ProjectMapGraph,
  ProjectMapNeighbors,
  ProjectMapOverview,
  ProjectMapStatus,
} from "../../types";
import {
  ProjectMapDebugToggle,
  ProjectMapDebugView,
  readProjectMapDebugButtonVisible,
  readProjectMapDebugEnabled,
  writeProjectMapDebugEnabled,
} from "./ProjectMapDebugView";

interface Props {
  projectPath?: string | null;
  onStatusChange?: (status: ProjectMapStatus) => void;
}

function formatTime(ms: number | null): string {
  if (!ms) return "未更新";
  const diff = Date.now() - ms;
  const minutes = Math.floor(diff / 60000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  return new Date(ms).toLocaleString();
}

function statusClass(status: string): string {
  if (status === "ready") return "bg-green-500";
  if (status === "updating") return "bg-blue-500 animate-pulse";
  if (status === "stale") return "bg-amber-500";
  if (status === "failed") return "bg-red-500";
  return "bg-gray-400";
}

function statusLabel(status: string): string {
  switch (status) {
    case "missing": return "未生成";
    case "ready": return "已就绪";
    case "stale": return "待更新";
    case "updating": return "更新中";
    case "failed": return "失败";
    default: return status;
  }
}

function complexityClass(complexity: string): string {
  if (complexity === "complex") return "text-red-500 bg-red-50 border-red-100";
  if (complexity === "moderate") return "text-amber-600 bg-amber-50 border-amber-100";
  return "text-text-secondary bg-sidebar-bg border-border-theme";
}

type PanelMode = "graph" | "list";

const GRAPH_MIN_WIDTH = 1000;
const GRAPH_MIN_HEIGHT = 640;
const MIN_GRAPH_ZOOM = 0.05;
const MAX_GRAPH_ZOOM = 3;
const GRAPH_ZOOM_STEP = 1.25;
const GRAPH_RING_GAP = 140;
const GRAPH_NODE_GAP = 150;

type ProjectMapPanelCache = {
  version: 2;
  projectPath: string | null;
  cachedAt: number;
  overview: ProjectMapOverview;
  graph: ProjectMapGraph | null;
};

const PROJECT_MAP_PANEL_CACHE_PREFIX = "deepagent:project-map-panel:";

function projectMapCacheKey(projectPath?: string | null): string {
  return `${PROJECT_MAP_PANEL_CACHE_PREFIX}${encodeURIComponent(projectPath?.trim() || "__default__")}`;
}

function readProjectMapPanelCache(projectPath?: string | null): ProjectMapPanelCache | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(projectMapCacheKey(projectPath));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<ProjectMapPanelCache>;
    if (parsed.version !== 2 || !parsed.overview) return null;
    return parsed as ProjectMapPanelCache;
  } catch {
    return null;
  }
}

function writeProjectMapPanelCache(
  projectPath: string | null | undefined,
  overview: ProjectMapOverview,
  graph: ProjectMapGraph | null,
) {
  if (typeof window === "undefined") return;
  try {
    const cache: ProjectMapPanelCache = {
      version: 2,
      projectPath: projectPath ?? null,
      cachedAt: Date.now(),
      overview,
      graph,
    };
    window.localStorage.setItem(projectMapCacheKey(projectPath), JSON.stringify(cache));
  } catch {
    // Best-effort cache only; quota/private-mode failures should not block the panel.
  }
}

export function ProjectMapStatusBadge({
  status,
  onClick,
}: {
  status: ProjectMapStatus | null;
  onClick?: () => void;
}) {
  const label = status
    ? `项目地图：${statusLabel(status.status)}，${status.nodes} 个节点 / ${status.edges} 条边`
    : "项目地图：加载中";
  return (
    <HoverInfo content={label}><button
      type="button"
      className="w-7 h-7 rounded-md flex items-center justify-center hover:bg-hover-bg transition-colors"

      onClick={onClick}
    >
      <span className={`w-2.5 h-2.5 rounded-full ${statusClass(status?.status ?? "loading")}`} />
    </button></HoverInfo>
  );
}

export function ProjectMapPanel({ projectPath, onStatusChange }: Props) {
  const [overview, setOverview] = useState<ProjectMapOverview | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<ProjectMapHit[]>([]);
  const [selected, setSelected] = useState<ProjectMapHit | null>(null);
  const [neighbors, setNeighbors] = useState<ProjectMapNeighbors | null>(null);
  const [graph, setGraph] = useState<ProjectMapGraph | null>(null);
  const [mode, setMode] = useState<PanelMode>("graph");
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [debugEnabled, setDebugEnabled] = useState(() => readProjectMapDebugEnabled());
  const [debugButtonVisible, setDebugButtonVisible] = useState(() => readProjectMapDebugButtonVisible());

  const updateDebugEnabled = (enabled: boolean) => {
    setDebugEnabled(enabled);
    writeProjectMapDebugEnabled(enabled);
  };
  const stats = overview?.status;
  const status = stats?.status ?? "missing";

  useEffect(() => {
    let cancelled = false;

    setSelected(null);
    setNeighbors(null);

    const cached = readProjectMapPanelCache(projectPath);
    if (cached) {
      setOverview(cached.overview);
      setGraph(cached.graph);
      setHits(cached.overview.complex_nodes);
      onStatusChange?.(cached.overview.status);
      setLoading(false);
      return () => {
        cancelled = true;
      };
    }

    const load = async () => {
      setLoading(true);
      try {
        const next = await projectMapOverview(projectPath);
        if (cancelled) return;
        setOverview(next);
        onStatusChange?.(next.status);
        setHits(next.complex_nodes);

        let graphNext: ProjectMapGraph | null = null;
        if (next.status.status !== "missing" && next.status.status !== "failed") {
          graphNext = await projectMapGraph(0, projectPath).catch(() => null);
          if (cancelled) return;
        }
        setGraph(graphNext);
        writeProjectMapPanelCache(projectPath, next, graphNext);
      } finally {
        if (!cancelled) setLoading(false);
      }
    };

    void load();
    return () => {
      cancelled = true;
    };
  }, [onStatusChange, projectPath]);

  useEffect(() => {
    const onDebugChanged = (event: Event) => {
      setDebugEnabled(Boolean((event as CustomEvent<boolean>).detail));
    };
    const onDebugButtonVisibleChanged = (event: Event) => {
      setDebugButtonVisible(Boolean((event as CustomEvent<boolean>).detail));
    };
    window.addEventListener("deepagent:project-map-debug-changed", onDebugChanged);
    window.addEventListener("deepagent:project-map-debug-button-visible-changed", onDebugButtonVisibleChanged);
    return () => {
      window.removeEventListener("deepagent:project-map-debug-changed", onDebugChanged);
      window.removeEventListener("deepagent:project-map-debug-button-visible-changed", onDebugButtonVisibleChanged);
    };
  }, []);

  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setHits(overview?.complex_nodes ?? []);
      return;
    }
    let cancelled = false;
    const handle = window.setTimeout(() => {
      projectMapSearch(q, 30, projectPath).then((items) => {
        if (!cancelled) setHits(items);
      });
    }, 180);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [overview?.complex_nodes, projectPath, query]);

  useEffect(() => {
    if (!selected) {
      setNeighbors(null);
      return;
    }
    let cancelled = false;
    projectMapNeighbors(selected.node_id, projectPath).then((next) => {
      if (!cancelled) setNeighbors(next);
    });
    return () => {
      cancelled = true;
    };
  }, [projectPath, selected]);

  const relationCount = useMemo(() => {
    if (!neighbors) return 0;
    return (
      neighbors.imports.length +
      neighbors.imported_by.length +
      neighbors.calls.length +
      neighbors.called_by.length +
      neighbors.related.length
    );
  }, [neighbors]);
  const hasComplexNodes = useMemo(
    () => (overview?.complex_nodes ?? []).some(
      (node) => node.complexity === "complex" || node.complexity === "moderate"
    ),
    [overview?.complex_nodes]
  );
  const listTitle = query.trim() ? "搜索结果" : hasComplexNodes ? "复杂模块" : "节点列表";
  const showDebugPanel = debugButtonVisible && debugEnabled;

  const handleRefresh = async () => {
    setRefreshing(true);
    try {
      await projectMapRefreshDeep(projectPath);
      const next = await projectMapOverview(projectPath);
      const graphNext = await projectMapGraph(0, projectPath).catch(() => null);
      setOverview(next);
      setGraph(graphNext);
      writeProjectMapPanelCache(projectPath, next, graphNext);
      onStatusChange?.(next.status);
      setHits(next.complex_nodes);
      setSelected(null);
    } catch (err) {
      console.error('Project map refresh failed:', err);
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <div className="h-full min-h-0 flex flex-col bg-bg-base">
      <div className="px-4 py-2 border-b border-border-theme flex-shrink-0">
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex shrink-0 items-center whitespace-nowrap">
            <Network className="mr-2 h-4 w-4 text-text-secondary" aria-hidden="true" />
            <div className="text-[14px] font-medium text-text-base">项目地图</div>

            {status !== "missing" && status !== "failed" && !showDebugPanel && (
              <div className="ml-3 inline-flex h-7 shrink-0 rounded-lg border border-border-theme bg-sidebar-bg p-0.5 text-[12px]">
                <button
                  type="button"
                  className={`px-2.5 rounded-md transition-colors ${mode === "graph" ? "bg-elevated-bg text-text-base shadow-sm" : "text-text-secondary hover:text-text-base"}`}
                  onClick={() => setMode("graph")}
                >
                  图谱
                </button>
                <button
                  type="button"
                  className={`px-2.5 rounded-md transition-colors ${mode === "list" ? "bg-elevated-bg text-text-base shadow-sm" : "text-text-secondary hover:text-text-base"}`}
                  onClick={() => setMode("list")}
                >
                  列表
                </button>
              </div>
            )}
          </div>
          <div className="ml-auto flex shrink-0 items-center gap-2 text-[12px] text-text-secondary">
            {debugButtonVisible && (
              <ProjectMapDebugToggle enabled={debugEnabled} onChange={updateDebugEnabled} />
            )}
            <HoverInfo content="使用 Understand-Anything 刷新地图"><button
              type="button"
              className="flex h-7 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-md border border-border-theme bg-elevated-bg px-2.5 text-text-base shadow-sm transition-colors hover:bg-hover-bg disabled:opacity-50"
              onClick={handleRefresh}
              disabled={refreshing}

            >
              <RotateCw className={`h-3.5 w-3.5 ${refreshing ? "animate-spin" : "text-text-secondary"}`} aria-hidden="true" />
              <span className="text-[11px] font-medium">刷新</span>
            </button></HoverInfo>
          </div>
        </div>

        <div className="mt-2 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-[11px] text-text-secondary">
          <span className="flex shrink-0 items-center whitespace-nowrap">
            <span className={`mr-1.5 h-2 w-2 rounded-full ${statusClass(refreshing ? "updating" : status)}`} />
            {refreshing ? "生成中" : loading ? "加载中" : statusLabel(status)}
          </span>
          <span className="whitespace-nowrap"><span className="font-medium text-text-base">{stats?.nodes ?? 0}</span> 节点</span>
          {mode === "graph" && graph && <span className="whitespace-nowrap">图中 {graph.nodes.length}</span>}
          <span className="whitespace-nowrap"><span className="font-medium text-text-base">{stats?.edges ?? 0}</span> 边</span>
          <span className="whitespace-nowrap"><span className="font-medium text-text-base">{stats?.files ?? 0}</span> 文件</span>
          <span className="whitespace-nowrap">更新于 {formatTime(stats?.updated_at ?? null)}</span>
        </div>

        {/* notice 提示已隐藏 */}
      </div>

      {showDebugPanel ? (
        <div className="flex-1 min-h-0 overflow-y-auto custom-scrollbar p-4">
          <ProjectMapDebugView projectPath={projectPath} compact />
        </div>
      ) : status === "missing" || status === "failed" ? (
        <div className="flex-1 min-h-0 overflow-y-auto p-5 text-[13px] text-text-secondary leading-6">
          <div className="rounded-xl border border-border-theme bg-sidebar-bg p-4">
            {status === "missing"
              ? "当前项目还没有项目地图。点击右上角刷新按钮可生成 Understand-Anything 完整项目地图。"
              : stats?.last_error ?? "项目地图加载失败。"}
          </div>
        </div>
      ) : (
        <>
          {mode === "graph" ? (
            <div className="project-map-view flex-1 min-h-0 flex flex-col">
              <ProjectMapGraphView
                graph={graph}
                selected={selected}
                onSelect={setSelected}
              />
            </div>
          ) : (
            <div className="project-map-view flex-1 min-h-0 grid grid-cols-[260px_1fr]">
              <div className="border-r border-border-theme min-h-0 flex flex-col">
                <div className="p-3 flex-shrink-0">
                  <div className="relative">
                    <Search className="absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-text-secondary" aria-hidden="true" />
                    <input
                      value={query}
                      onChange={(e) => setQuery(e.target.value)}
                      placeholder="搜索文件、函数、模块"
                      className="w-full h-9 rounded-lg border border-border-theme pl-8 pr-3 text-[13px] outline-none focus:border-primary"
                    />
                  </div>
                </div>
                <div className="px-3 pb-2 text-[12px] font-medium text-text-secondary">
                  {listTitle}
                </div>
                <div className="flex-1 min-h-0 overflow-y-auto custom-scrollbar px-2 pb-3">
                  {hits.map((hit) => (
                    <button
                      key={hit.node_id}
                      className={`w-full text-left rounded-lg px-3 py-2 mb-1 transition-colors ${
                        selected?.node_id === hit.node_id
                          ? "bg-hover-bg text-text-base"
                          : "hover:bg-hover-bg text-text-secondary"
                      }`}
                      onClick={() => setSelected(hit)}
                    >
                      <div className="flex items-start justify-between gap-2">
                        <div className="flex items-center gap-2 min-w-0 pt-0.5">
                          {hit.node_type === "function" ? (
                            <Code2 className="h-3.5 w-3.5 flex-shrink-0" aria-hidden="true" />
                          ) : hit.node_type === "class" ? (
                            <Box className="h-3.5 w-3.5 flex-shrink-0" aria-hidden="true" />
                          ) : (
                            <FileText className="h-3.5 w-3.5 flex-shrink-0" aria-hidden="true" />
                          )}
                          <span className="text-[13px] font-medium text-text-base truncate">{hit.name}</span>
                        </div>
                        <span className={`text-[10px] border rounded px-1.5 py-0.5 whitespace-nowrap flex-shrink-0 ${complexityClass(hit.complexity)}`}>
                          {translateComplexity(hit.complexity)}
                        </span>
                      </div>
                      <div className="mt-1 text-[11px] truncate text-text-secondary">
                        {hit.file_path ?? translateNodeType(hit.node_type)}
                      </div>
                    </button>
                  ))}
                  {hits.length === 0 && (
                    <div className="px-3 py-8 text-center text-[13px] text-text-secondary">
                      {query.trim() ? "没有匹配节点" : "没有可显示节点"}
                    </div>
                  )}
                </div>
              </div>

              <div className="min-h-0 overflow-y-auto custom-scrollbar p-4">
                {selected ? (
                  <div>
                    <div className="flex items-start justify-between gap-3">
                      <div className="min-w-0">
                        <div className="text-[16px] font-medium text-text-base truncate">{selected.name}</div>
                        <div className="mt-1 text-[12px] text-text-secondary truncate">
                          {selected.file_path ?? selected.node_id}
                        </div>
                      </div>
                      <span className={`text-[11px] border rounded px-2 py-1 whitespace-nowrap flex-shrink-0 ${complexityClass(selected.complexity)}`}>
                        {translateComplexity(selected.complexity)}
                      </span>
                    </div>

                    {selected.summary && (
                      <div className="mt-4 text-[13px] leading-6 text-text-base rounded-xl border border-border-theme bg-sidebar-bg px-3 py-2">
                        {selected.summary}
                      </div>
                    )}

                    <div className="mt-4 text-[12px] text-text-secondary">
                      关系数量：{relationCount}
                    </div>

                    <RelationBlock title="导入了" items={neighbors?.imports ?? []} onSelect={setSelected} />
                    <RelationBlock title="被导入" items={neighbors?.imported_by ?? []} onSelect={setSelected} />
                    <RelationBlock title="调用了" items={neighbors?.calls ?? []} onSelect={setSelected} />
                    <RelationBlock title="被调用" items={neighbors?.called_by ?? []} onSelect={setSelected} />
                    <RelationBlock title="相关联" items={neighbors?.related ?? []} onSelect={setSelected} />
                  </div>
                ) : (
                  <div className="h-full flex items-center justify-center text-[13px] text-text-secondary">
                    选择一个节点查看详情
                  </div>
                )}
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}

function ProjectMapGraphView({
  graph,
  selected,
  onSelect,
}: {
  graph: ProjectMapGraph | null;
  selected: ProjectMapHit | null;
  onSelect: (hit: ProjectMapHit | null) => void;
}) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const frameRef = useRef<number | null>(null);
  const viewportSizeRef = useRef({ width: 0, height: 0 });
  const viewRef = useRef({ x: 0, y: 0, k: 1 });
  const [zoomPercent, setZoomPercent] = useState(100);
  const panRef = useRef<{ pointerId: number; startX: number; startY: number; x: number; y: number; moved: boolean } | null>(null);
  const layout = useMemo(() => {
    const nodes = graph?.nodes ?? [];
    const edges = graph?.edges ?? [];
    const positions = new Map<string, { x: number; y: number }>();
    if (nodes[0]) positions.set(nodes[0].node_id, { x: 0, y: 0 });
    let placed = 1;
    let ring = 1;
    while (placed < nodes.length) {
      const radius = ring * GRAPH_RING_GAP;
      const count = Math.min(nodes.length - placed, Math.max(6, Math.floor(2 * Math.PI * radius / GRAPH_NODE_GAP)));
      for (let i = 0; i < count; i++) {
        const angle = -Math.PI / 2 + (i / count) * 2 * Math.PI;
        positions.set(nodes[placed + i].node_id, {
          x: Math.cos(angle) * radius,
          y: Math.sin(angle) * radius,
        });
      }
      placed += count;
      ring++;
    }
    const extent = Math.max(GRAPH_MIN_HEIGHT / 2, (ring - 1) * GRAPH_RING_GAP + 90);
    const width = Math.max(GRAPH_MIN_WIDTH, extent * 2);
    const height = Math.max(GRAPH_MIN_HEIGHT, extent * 2);
    const offsetX = width / 2;
    const offsetY = height / 2;
    for (const position of positions.values()) {
      position.x += offsetX;
      position.y += offsetY;
    }
    return { nodes, edges, positions, width, height };
  }, [graph]);

  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const { width, height } = viewportSizeRef.current;
    if (!canvas || width <= 0 || height <= 0) return;
    const dpr = window.devicePixelRatio || 1;
    const bitmapWidth = Math.round(width * dpr);
    const bitmapHeight = Math.round(height * dpr);
    if (canvas.width !== bitmapWidth || canvas.height !== bitmapHeight) {
      canvas.width = bitmapWidth;
      canvas.height = bitmapHeight;
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);

    const view = viewRef.current;
    const left = -view.x / view.k - 150;
    const top = -view.y / view.k - 150;
    const right = (width - view.x) / view.k + 150;
    const bottom = (height - view.y) / view.k + 150;
    const detailed = view.k >= 0.6;
    ctx.save();
    ctx.translate(view.x, view.y);
    ctx.scale(view.k, view.k);

    for (const edge of layout.edges) {
      const source = layout.positions.get(edge.source);
      const target = layout.positions.get(edge.target);
      if (!source || !target) continue;
      if (Math.max(source.x, target.x) < left || Math.min(source.x, target.x) > right
        || Math.max(source.y, target.y) < top || Math.min(source.y, target.y) > bottom) continue;
      ctx.strokeStyle = edgeColor(edge.edge_type);
      ctx.globalAlpha = selected && edge.source !== selected.node_id && edge.target !== selected.node_id
        ? 0.12 : detailed ? 0.58 : 0.3;
      ctx.lineWidth = edge.edge_type === "calls" ? 1.8 : 1.2;
      ctx.beginPath();
      ctx.moveTo(source.x, source.y);
      ctx.lineTo(target.x, target.y);
      ctx.stroke();
    }
    ctx.globalAlpha = 1;

    for (const node of layout.nodes) {
      const position = layout.positions.get(node.node_id);
      if (!position || position.x < left || position.x > right || position.y < top || position.y > bottom) continue;
      const isSelected = selected?.node_id === node.node_id;
      if (!detailed && !isSelected) {
        ctx.fillStyle = nodeAccent(node.node_type);
        ctx.beginPath();
        ctx.arc(position.x, position.y, Math.max(9, 2.5 / view.k), 0, Math.PI * 2);
        ctx.fill();
        continue;
      }

      const nodeWidth = node.node_type === "function" ? 118 : 136;
      const x = position.x - nodeWidth / 2;
      const y = position.y - 22;
      ctx.beginPath();
      ctx.roundRect(x, y, nodeWidth, 44, 8);
      ctx.fillStyle = isSelected ? "#111827" : nodeFill(node.node_type);
      ctx.fill();
      ctx.strokeStyle = isSelected ? "#111827" : nodeStroke(node.node_type);
      ctx.lineWidth = isSelected ? 2 : 1;
      ctx.stroke();
      ctx.beginPath();
      ctx.arc(x + 17, y + 22, 5, 0, Math.PI * 2);
      ctx.fillStyle = isSelected ? "#ffffff" : nodeAccent(node.node_type);
      ctx.fill();
      ctx.font = `${isSelected ? 700 : 600} 12px sans-serif`;
      ctx.fillStyle = isSelected ? "#ffffff" : "#172033";
      ctx.fillText(shortLabel(node.name, node.node_type === "function" ? 12 : 15), x + 30, y + 19);
      ctx.font = "9px sans-serif";
      ctx.fillStyle = isSelected ? "#d1d5db" : "#667085";
      ctx.fillText(translateNodeType(node.node_type), x + 30, y + 34);
    }
    ctx.restore();
  }, [layout, selected]);
  const drawRef = useRef(draw);
  drawRef.current = draw;
  const scheduleDraw = useCallback(() => {
    if (frameRef.current !== null) return;
    frameRef.current = window.requestAnimationFrame(() => {
      frameRef.current = null;
      drawRef.current();
    });
  }, []);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const measure = () => {
      const { width, height } = viewport.getBoundingClientRect();
      const previous = viewportSizeRef.current;
      if (width === previous.width && height === previous.height) return;
      viewportSizeRef.current = { width, height };
      const view = viewRef.current;
      viewRef.current = previous.width === 0 && previous.height === 0
        ? { ...view, x: (width - layout.width * view.k) / 2, y: (height - layout.height * view.k) / 2 }
        : { ...view, x: view.x + (width - previous.width) / 2, y: view.y + (height - previous.height) / 2 };
      scheduleDraw();
    };
    measure();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(viewport);
    window.addEventListener("resize", measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [graph, layout.width, layout.height, scheduleDraw]);

  useEffect(() => {
    if (!graph) return;
    const { width, height } = viewportSizeRef.current;
    const k = Math.max(MIN_GRAPH_ZOOM, Math.min(1, Math.min(width / layout.width, height / layout.height) * 0.95));
    viewRef.current = { x: (width - layout.width * k) / 2, y: (height - layout.height * k) / 2, k };
    setZoomPercent(Math.round(k * 100));
    scheduleDraw();
  }, [graph, layout.width, layout.height, scheduleDraw]);

  useEffect(() => {
    scheduleDraw();
  }, [scheduleDraw, selected]);

  useEffect(() => () => {
    if (frameRef.current !== null) {
      window.cancelAnimationFrame(frameRef.current);
      frameRef.current = null;
    }
  }, []);

  const zoomAt = useCallback((factor: number, anchor: { x: number; y: number }) => {
    const current = viewRef.current;
    const k = Math.max(MIN_GRAPH_ZOOM, Math.min(MAX_GRAPH_ZOOM, current.k * factor));
    if (k === current.k) return;
    const worldX = (anchor.x - current.x) / current.k;
    const worldY = (anchor.y - current.y) / current.k;
    viewRef.current = { x: anchor.x - worldX * k, y: anchor.y - worldY * k, k };
    setZoomPercent(Math.round(k * 100));
    scheduleDraw();
  }, [scheduleDraw]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const handleWheel = (event: WheelEvent) => {
      event.preventDefault();
      const rect = canvas.getBoundingClientRect();
      zoomAt(event.deltaY < 0 ? GRAPH_ZOOM_STEP : 1 / GRAPH_ZOOM_STEP, {
        x: event.clientX - rect.left,
        y: event.clientY - rect.top,
      });
    };
    canvas.addEventListener("wheel", handleWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", handleWheel);
  }, [graph, zoomAt]);

  const zoomAtCenter = (factor: number) => {
    const { width, height } = viewportSizeRef.current;
    zoomAt(factor, { x: width / 2, y: height / 2 });
  };

  const resetView = () => {
    const { width, height } = viewportSizeRef.current;
    viewRef.current = { x: (width - layout.width) / 2, y: (height - layout.height) / 2, k: 1 };
    setZoomPercent(100);
    scheduleDraw();
  };

  const fitView = () => {
    const { width, height } = viewportSizeRef.current;
    if (width === 0 || height === 0) return;
    const k = Math.max(MIN_GRAPH_ZOOM, Math.min(MAX_GRAPH_ZOOM,
      Math.min(width / layout.width, height / layout.height) * 0.95));
    viewRef.current = { x: (width - layout.width * k) / 2, y: (height - layout.height * k) / 2, k };
    setZoomPercent(Math.round(k * 100));
    scheduleDraw();
  };

  const handlePointerDown = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    if (event.button !== 0) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const view = viewRef.current;
    panRef.current = { pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, x: view.x, y: view.y, moved: false };
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const pan = panRef.current;
    if (!pan || pan.pointerId !== event.pointerId) return;
    const dx = event.clientX - pan.startX;
    const dy = event.clientY - pan.startY;
    if (Math.abs(dx) + Math.abs(dy) > 3) pan.moved = true;
    viewRef.current = { ...viewRef.current, x: pan.x + dx, y: pan.y + dy };
    scheduleDraw();
  };

  const handlePointerUp = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const pan = panRef.current;
    if (!pan || pan.pointerId !== event.pointerId) return;
    panRef.current = null;
    event.currentTarget.releasePointerCapture(event.pointerId);
    if (pan.moved) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const view = viewRef.current;
    const x = (event.clientX - rect.left - view.x) / view.k;
    const y = (event.clientY - rect.top - view.y) / view.k;
    const hit = [...layout.nodes].reverse().find((node) => {
      const position = layout.positions.get(node.node_id);
      if (!position) return false;
      if (view.k < 0.6 && node.node_id !== selected?.node_id) {
        return Math.hypot(x - position.x, y - position.y) <= Math.max(9, 4 / view.k);
      }
      const width = node.node_type === "function" ? 118 : 136;
      return Math.abs(x - position.x) <= width / 2 && Math.abs(y - position.y) <= 22;
    });
    onSelect(hit ?? null);
  };

  if (!graph) {
    return (
      <div className="flex-1 min-h-0 flex items-center justify-center text-[13px] text-text-secondary">
        图谱加载中
      </div>
    );
  }

  if (layout.nodes.length === 0) {
    return (
      <div className="flex-1 min-h-0 flex items-center justify-center text-[13px] text-text-secondary">
        没有可展示的图谱节点
      </div>
    );
  }

  return (
    <div className="relative min-h-0 flex-1 bg-[#fbfcfd]">
      <div ref={viewportRef} className="absolute inset-0 overflow-hidden" role="region" aria-label="项目地图画布" tabIndex={0}>
        <canvas
          ref={canvasRef}
          className="block h-full w-full touch-none select-none cursor-grab active:cursor-grabbing"
          role="img"
          aria-label="项目关系图谱"
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={() => { panRef.current = null; }}
        />
      </div>

      <div className="absolute bottom-4 right-4 z-10 flex items-center gap-1 rounded-xl border border-border-theme bg-elevated-bg/95 p-1 shadow-sm">
        <HoverInfo content="缩小">
          <Button variant="ghost" size="icon" className="h-7 w-7" aria-label="缩小项目地图" disabled={zoomPercent <= MIN_GRAPH_ZOOM * 100} onClick={() => zoomAtCenter(1 / GRAPH_ZOOM_STEP)}>
            <ZoomOut className="h-4 w-4" aria-hidden="true" />
          </Button>
        </HoverInfo>
        <HoverInfo content="重置为 100%">
          <Button variant="ghost" size="sm" className="h-7 min-w-[48px] px-1.5" aria-label="重置项目地图缩放" onClick={resetView}>
            {zoomPercent}%
          </Button>
        </HoverInfo>
        <HoverInfo content="放大">
          <Button variant="ghost" size="icon" className="h-7 w-7" aria-label="放大项目地图" disabled={zoomPercent >= MAX_GRAPH_ZOOM * 100} onClick={() => zoomAtCenter(GRAPH_ZOOM_STEP)}>
            <ZoomIn className="h-4 w-4" aria-hidden="true" />
          </Button>
        </HoverInfo>
        <HoverInfo content="适应画布">
          <Button variant="ghost" size="icon" className="h-7 w-7" aria-label="适应项目地图画布" onClick={fitView}>
            <Maximize className="h-4 w-4" aria-hidden="true" />
          </Button>
        </HoverInfo>
      </div>

      {selected && (
          <div className="popover-menu absolute right-4 top-4 w-[280px] rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] border border-border-theme bg-elevated-bg/95 backdrop-blur-md p-4 flex flex-col max-h-[calc(100%-32px)] overflow-y-auto custom-scrollbar z-10">
            <div className="text-[12px] font-medium text-text-secondary mb-3">当前节点信息</div>
            <div className="text-[14px] font-medium text-text-base break-words">{selected.name}</div>
            <div className="mt-1 text-[11px] text-text-secondary break-all">
              {selected.file_path ?? selected.node_id}
            </div>
            <div className="mt-3 flex items-center gap-2">
              <span className="text-[10px] border rounded px-1.5 py-0.5 text-text-secondary bg-sidebar-bg">
                {translateNodeType(selected.node_type)}
              </span>
              <span className={`text-[10px] border rounded px-1.5 py-0.5 whitespace-nowrap flex-shrink-0 ${complexityClass(selected.complexity)}`}>
                {translateComplexity(selected.complexity)}
              </span>
            </div>
            {selected.summary && (
              <div className="mt-3 text-[12px] leading-5 text-text-secondary bg-sidebar-bg/50 p-2 rounded-lg border border-border-theme/50">
                {selected.summary}
              </div>
            )}
          </div>
        )}
    </div>
  );
}

function shortLabel(value: string, limit: number): string {
  return value.length > limit ? `${value.slice(0, limit - 1)}...` : value;
}

function nodeFill(type: string): string {
  if (type === "class") return "#eef6ff";
  if (type === "function") return "#effaf3";
  if (type === "endpoint") return "#fff7ed";
  if (type === "service") return "#f5f3ff";
  return "#ffffff";
}

function nodeStroke(type: string): string {
  if (type === "class") return "#9cc8ff";
  if (type === "function") return "#9ed8b2";
  if (type === "endpoint") return "#fdba74";
  if (type === "service") return "#c4b5fd";
  return "#d9dee8";
}

function nodeAccent(type: string): string {
  if (type === "class") return "#2f80ed";
  if (type === "function") return "#22a06b";
  if (type === "endpoint") return "#f97316";
  if (type === "service") return "#7c3aed";
  return "#667085";
}

function edgeColor(type: string): string {
  if (type === "calls") return "#2563eb";
  if (type === "imports") return "#7c3aed";
  if (type === "contains") return "#64748b";
  if (type === "routes") return "#f97316";
  return "#94a3b8";
}

function RelationBlock({
  title,
  items,
  onSelect,
}: {
  title: string;
  items: { node: ProjectMapHit }[];
  onSelect: (hit: ProjectMapHit) => void;
}) {
  if (items.length === 0) return null;
  return (
    <div className="mt-4">
      <div className="mb-2 text-[12px] font-medium text-text-secondary">{title}</div>
      <div className="space-y-1">
        {items.map((item) => (
          <button
            key={`${title}:${item.node.node_id}`}
            className="w-full flex items-center justify-between gap-3 rounded-lg border border-border-theme px-3 py-2 text-left hover:bg-hover-bg transition-colors"
            onClick={() => onSelect(item.node)}
          >
            <div className="min-w-0">
              <div className="text-[13px] text-text-base truncate">{item.node.name}</div>
              <div className="text-[11px] text-text-secondary truncate">
                {item.node.file_path ?? translateNodeType(item.node.node_type)}
              </div>
            </div>
            <ChevronRight className="h-3.5 w-3.5 text-text-secondary" aria-hidden="true" />
          </button>
        ))}
      </div>
    </div>
  );
}

const typeMap: Record<string, string> = {
  class: "类",
  function: "函数",
  file: "文件",
  service: "服务",
  endpoint: "接口",
  method: "方法",
};
function translateNodeType(type: string): string {
  return typeMap[type] || type;
}

const complexityMap: Record<string, string> = {
  complex: "复杂",
  moderate: "中等",
  simple: "简单",
};
function translateComplexity(comp: string): string {
  return complexityMap[comp] || comp;
}
