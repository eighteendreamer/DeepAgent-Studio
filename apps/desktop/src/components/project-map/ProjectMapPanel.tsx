import { useEffect, useMemo, useState } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
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

function complexityClass(complexity: string): string {
  if (complexity === "complex") return "text-red-500 bg-red-50 border-red-100";
  if (complexity === "moderate") return "text-amber-600 bg-amber-50 border-amber-100";
  return "text-text-secondary bg-gray-50 border-border-theme";
}

type PanelMode = "graph" | "list";

export function ProjectMapStatusBadge({
  status,
  onClick,
}: {
  status: ProjectMapStatus | null;
  onClick?: () => void;
}) {
  const label = status
    ? `项目地图：${status.status}，${status.nodes} nodes / ${status.edges} edges`
    : "项目地图：加载中";
  return (
    <button
      type="button"
      className="w-7 h-7 rounded-md flex items-center justify-center hover:bg-gray-100 transition-colors"
      title={label}
      onClick={onClick}
    >
      <span className={`w-2.5 h-2.5 rounded-full ${statusClass(status?.status ?? "loading")}`} />
    </button>
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
  const [notice, setNotice] = useState<string | null>(null);
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
    setLoading(true);
    projectMapOverview(projectPath)
      .then((next) => {
        if (cancelled) return;
        setOverview(next);
        onStatusChange?.(next.status);
        setHits(next.complex_nodes);
        setSelected(null);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [onStatusChange, projectPath]);

  useEffect(() => {
    let cancelled = false;
    if (status === "missing" || status === "failed") {
      setGraph(null);
      return;
    }
    projectMapGraph(90, projectPath)
      .then((next) => {
        if (!cancelled) setGraph(next);
      })
      .catch(() => {
        if (!cancelled) setGraph(null);
      });
    return () => {
      cancelled = true;
    };
  }, [projectPath, status, stats?.nodes, stats?.edges, stats?.updated_at]);

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
    setNotice(null);
    try {
      const result = await projectMapRefreshDeep(projectPath);
      const next = await projectMapOverview(projectPath);
      const graphNext = await projectMapGraph(90, projectPath).catch(() => null);
      setOverview(next);
      setGraph(graphNext);
      onStatusChange?.(next.status);
      setHits(next.complex_nodes);
      setSelected(null);
      setNotice(
        `${result.message} ${result.nodes} 个节点 / ${result.edges} 条关系，耗时 ${result.duration_ms}ms。`
      );
    } catch (err) {
      setNotice(err instanceof Error ? err.message : String(err));
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <div className="h-full flex flex-col bg-white">
      <div className="px-4 py-2 border-b border-border-theme flex-shrink-0">
        <div className="flex items-center justify-between">
          <div className="flex items-center min-w-0">
            <FontAwesomeIcon icon={["fas", "share-nodes"]} className="text-text-secondary mr-2" />
            <div className="text-[14px] font-medium text-text-base">项目地图</div>
            
            {status !== "missing" && status !== "failed" && !showDebugPanel && (
              <div className="ml-4 inline-flex h-7 rounded-lg border border-border-theme bg-gray-50 p-0.5 text-[12px]">
                <button
                  type="button"
                  className={`px-3 rounded-md transition-colors ${mode === "graph" ? "bg-white text-text-base shadow-sm" : "text-text-secondary hover:text-text-base"}`}
                  onClick={() => setMode("graph")}
                >
                  图谱
                </button>
                <button
                  type="button"
                  className={`px-3 rounded-md transition-colors ${mode === "list" ? "bg-white text-text-base shadow-sm" : "text-text-secondary hover:text-text-base"}`}
                  onClick={() => setMode("list")}
                >
                  列表
                </button>
              </div>
            )}
          </div>
          <div className="flex items-center gap-3 text-[12px] text-text-secondary">
            {debugButtonVisible && (
              <ProjectMapDebugToggle enabled={debugEnabled} onChange={updateDebugEnabled} />
            )}
            <div className="flex items-center">
              <span className={`w-2 h-2 rounded-full mr-1.5 ${statusClass(refreshing ? "updating" : status)}`} />
              {refreshing ? "生成中" : loading ? "加载中" : status}
            </div>
            <button
              type="button"
              className="h-7 px-2.5 rounded-md border border-border-theme bg-white hover:bg-gray-50 text-text-base transition-colors disabled:opacity-50 flex items-center gap-1.5 shadow-sm"
              onClick={handleRefresh}
              disabled={refreshing}
              title="使用 Understand-Anything 刷新地图"
            >
              <FontAwesomeIcon
                icon={["fas", "rotate-right"]}
                className={`text-[11px] ${refreshing ? "animate-spin" : "text-text-secondary"}`}
              />
              <span className="text-[11px] font-medium">刷新</span>
            </button>
          </div>
        </div>
        
        <div className="mt-2.5 flex items-center justify-between text-[11px] text-text-secondary">
          <div className="flex items-center gap-2.5">
            <span className="font-medium text-text-base">{stats?.nodes ?? 0}</span> 节点
            <span className="w-1 h-1 rounded-full bg-border-theme"></span>
            <span className="font-medium text-text-base">{stats?.edges ?? 0}</span> 边
            <span className="w-1 h-1 rounded-full bg-border-theme"></span>
            <span className="font-medium text-text-base">{stats?.files ?? 0}</span> 文件
            {stats?.source && (
              <>
                <span className="w-1 h-1 rounded-full bg-border-theme"></span>
                <span className="truncate max-w-[120px]" title={stats?.graph_path ?? undefined}>{stats.source}</span>
              </>
            )}
          </div>
          <span>更新于 {formatTime(stats?.updated_at ?? null)}</span>
        </div>

        {notice && (
          <div className="mt-2 rounded-md border border-border-theme bg-gray-50 px-2 py-1.5 text-[11px] text-text-secondary">
            {notice}
          </div>
        )}
      </div>

      {showDebugPanel ? (
        <div className="flex-1 min-h-0 overflow-y-auto custom-scrollbar p-4">
          <ProjectMapDebugView projectPath={projectPath} compact />
        </div>
      ) : status === "missing" || status === "failed" ? (
        <div className="flex-1 p-5 text-[13px] text-text-secondary leading-6">
          <div className="rounded-xl border border-border-theme bg-gray-50 p-4">
            {status === "missing"
              ? "当前项目还没有项目地图。点击右上角刷新按钮可生成 Understand-Anything 完整项目地图。"
              : stats?.last_error ?? "项目地图加载失败。"}
          </div>
        </div>
      ) : (
        <AnimatePresence mode="wait">
          {mode === "graph" ? (
            <motion.div
              key="graph"
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -10 }}
              transition={{ duration: 0.15 }}
              className="flex-1 min-h-0 flex flex-col"
            >
              <ProjectMapGraphView
                graph={graph}
                selected={selected}
                onSelect={setSelected}
              />
            </motion.div>
          ) : (
            <motion.div
              key="list"
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -10 }}
              transition={{ duration: 0.15 }}
              className="flex-1 min-h-0 grid grid-cols-[260px_1fr]"
            >
              <div className="border-r border-border-theme min-h-0 flex flex-col">
                <div className="p-3 flex-shrink-0">
                  <div className="relative">
                    <FontAwesomeIcon
                      icon={["fas", "magnifying-glass"]}
                      className="absolute left-3 top-1/2 -translate-y-1/2 text-[12px] text-text-secondary"
                    />
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
                          ? "bg-gray-100 text-text-base"
                          : "hover:bg-gray-50 text-text-secondary"
                      }`}
                      onClick={() => setSelected(hit)}
                    >
                      <div className="flex items-start justify-between gap-2">
                        <div className="flex items-center gap-2 min-w-0 pt-0.5">
                          <FontAwesomeIcon
                            icon={
                              hit.node_type === "function"
                                ? ["fas", "code"]
                                : hit.node_type === "class"
                                  ? ["fas", "cube"]
                                  : ["far", "file-lines"]
                            }
                            className="text-[11px] flex-shrink-0"
                          />
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
                      <div className="mt-4 text-[13px] leading-6 text-text-base rounded-xl border border-border-theme bg-gray-50 px-3 py-2">
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
            </motion.div>
          )}
        </AnimatePresence>
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
  const layout = useMemo(() => {
    const nodes = graph?.nodes ?? [];
    const edges = graph?.edges ?? [];
    const selectedId = selected?.node_id ?? nodes[0]?.node_id ?? "";
    const visibleNodes = nodes.slice(0, 90);
    const byId = new Map(visibleNodes.map((node) => [node.node_id, node]));
    const visibleEdges = edges.filter((edge) => byId.has(edge.source) && byId.has(edge.target)).slice(0, 220);
    const connected = new Set<string>([selectedId]);
    for (const edge of visibleEdges) {
      if (edge.source === selectedId) connected.add(edge.target);
      if (edge.target === selectedId) connected.add(edge.source);
    }

    const ordered = [...visibleNodes].sort((a, b) => {
      if (a.node_id === selectedId) return -1;
      if (b.node_id === selectedId) return 1;
      const ca = connected.has(a.node_id) ? 0 : 1;
      const cb = connected.has(b.node_id) ? 0 : 1;
      return ca - cb || typeRank(a.node_type) - typeRank(b.node_type) || a.name.localeCompare(b.name);
    });
    const positions = new Map<string, { x: number; y: number }>();
    if (ordered[0]) positions.set(ordered[0].node_id, { x: 500, y: 315 });
    const rings = [
      { radiusX: 230, radiusY: 145, start: 1, count: Math.min(18, Math.max(0, ordered.length - 1)) },
      { radiusX: 390, radiusY: 245, start: 19, count: Math.min(36, Math.max(0, ordered.length - 19)) },
      { radiusX: 470, radiusY: 300, start: 55, count: Math.max(0, ordered.length - 55) },
    ];
    for (const ring of rings) {
      for (let i = 0; i < ring.count; i++) {
        const node = ordered[ring.start + i];
        if (!node) continue;
        const angle = -Math.PI / 2 + (i / Math.max(1, ring.count)) * Math.PI * 2;
        positions.set(node.node_id, {
          x: 500 + Math.cos(angle) * ring.radiusX,
          y: 315 + Math.sin(angle) * ring.radiusY,
        });
      }
    }
    return { nodes: ordered, edges: visibleEdges, positions };
  }, [graph, selected?.node_id]);

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
    <div className="flex-1 min-h-0 relative bg-[#fbfcfd]">
      <svg
        viewBox="0 0 1000 640"
        className="w-full h-full block cursor-default"
        role="img"
        aria-label="项目关系图谱"
        onClick={() => onSelect(null)}
      >
        <defs>
          <marker id="project-map-arrow" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto">
            <path d="M0,0 L8,4 L0,8 Z" fill="#9aa4b2" />
          </marker>
        </defs>
        {layout.edges.map((edge, index) => {
          const source = layout.positions.get(edge.source);
          const target = layout.positions.get(edge.target);
          if (!source || !target) return null;
          return (
            <line
              key={`${edge.source}:${edge.target}:${edge.edge_type}:${index}`}
              x1={source.x}
              y1={source.y}
              x2={target.x}
              y2={target.y}
              stroke={edgeColor(edge.edge_type)}
              strokeWidth={edge.edge_type === "calls" ? 1.8 : 1.2}
              strokeOpacity={selected && edge.source !== selected.node_id && edge.target !== selected.node_id ? 0.22 : 0.58}
              markerEnd="url(#project-map-arrow)"
            />
          );
        })}
        {layout.nodes.map((node) => {
          const position = layout.positions.get(node.node_id);
          if (!position) return null;
          const isSelected = selected?.node_id === node.node_id;
          const width = node.node_type === "function" ? 118 : 136;
          return (
            <g
              key={node.node_id}
              transform={`translate(${position.x - width / 2} ${position.y - 22})`}
              className="cursor-pointer"
              onClick={(e) => {
                e.stopPropagation();
                onSelect(node);
              }}
            >
              <rect
                width={width}
                height="44"
                rx="8"
                fill={isSelected ? "#111827" : nodeFill(node.node_type)}
                stroke={isSelected ? "#111827" : nodeStroke(node.node_type)}
                strokeWidth={isSelected ? 2 : 1}
              />
              <circle cx="17" cy="22" r="5" fill={isSelected ? "#ffffff" : nodeAccent(node.node_type)} />
              <text
                x="30"
                y="19"
                fontSize="12"
                fontWeight={isSelected ? 700 : 600}
                fill={isSelected ? "#ffffff" : "#172033"}
              >
                {shortLabel(node.name, node.node_type === "function" ? 12 : 15)}
              </text>
              <text x="30" y="34" fontSize="9" fill={isSelected ? "#d1d5db" : "#667085"}>
                {translateNodeType(node.node_type)}
              </text>
            </g>
          );
        })}
      </svg>

      <AnimatePresence>
        {selected && (
          <motion.div
            initial={{ opacity: 0, y: -10, scale: 0.95 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -10, scale: 0.95 }}
            transition={{ duration: 0.15, ease: "easeOut" }}
            className="absolute right-4 top-4 w-[280px] rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] border border-border-theme bg-white/95 backdrop-blur-md p-4 flex flex-col max-h-[calc(100%-32px)] overflow-y-auto custom-scrollbar z-10"
          >
            <div className="text-[12px] font-medium text-text-secondary mb-3">当前节点信息</div>
            <div className="text-[14px] font-medium text-text-base break-words">{selected.name}</div>
            <div className="mt-1 text-[11px] text-text-secondary break-all">
              {selected.file_path ?? selected.node_id}
            </div>
            <div className="mt-3 flex items-center gap-2">
              <span className="text-[10px] border rounded px-1.5 py-0.5 text-text-secondary bg-gray-50">
                {translateNodeType(selected.node_type)}
              </span>
              <span className={`text-[10px] border rounded px-1.5 py-0.5 whitespace-nowrap flex-shrink-0 ${complexityClass(selected.complexity)}`}>
                {translateComplexity(selected.complexity)}
              </span>
            </div>
            {selected.summary && (
              <div className="mt-3 text-[12px] leading-5 text-text-secondary bg-gray-50/50 p-2 rounded-lg border border-border-theme/50">
                {selected.summary}
              </div>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

function typeRank(type: string): number {
  if (type === "class") return 0;
  if (type === "function") return 1;
  if (type === "endpoint") return 2;
  if (type === "service") return 3;
  if (type === "file") return 4;
  return 5;
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
            className="w-full flex items-center justify-between gap-3 rounded-lg border border-border-theme px-3 py-2 text-left hover:bg-gray-50 transition-colors"
            onClick={() => onSelect(item.node)}
          >
            <div className="min-w-0">
              <div className="text-[13px] text-text-base truncate">{item.node.name}</div>
              <div className="text-[11px] text-text-secondary truncate">
                {item.node.file_path ?? translateNodeType(item.node.node_type)}
              </div>
            </div>
            <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] text-text-secondary" />
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
