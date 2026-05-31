import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { KnowledgeEntry } from "../types";

// Accent color per entry kind (matches the badge palette used elsewhere).
const KIND_COLOR: Record<string, string> = {
  pitfall: "#dc2626", // red-600
  solution: "#16a34a", // green-600
  command: "#4b5563", // gray-600
  config: "#2563eb", // blue-600
  note: "#ca8a04", // yellow-600
};

const TAG_COLOR = "#94a3b8"; // slate-400

function kindColor(kind: string): string {
  return KIND_COLOR[kind] ?? "#6b7280";
}

interface SimNode {
  id: string;
  type: "entry" | "tag";
  label: string;
  entry?: KnowledgeEntry;
  tag?: string;
  degree: number;
  x: number;
  y: number;
  vx: number;
  vy: number;
  fx?: number | null; // pinned position while dragging
  fy?: number | null;
}

interface SimLink {
  source: string;
  target: string;
}

interface Transform {
  x: number;
  y: number;
  k: number;
}

interface Props {
  entries: KnowledgeEntry[];
  selectedId: string | null;
  search: string;
  onSelect: (entry: KnowledgeEntry) => void;
}

// --- force simulation constants -------------------------------------------
const REPULSION = 1600;
const LINK_DISTANCE = 70;
const LINK_STRENGTH = 0.04;
const CENTER_GRAVITY = 0.015;
const DAMPING = 0.86;
const ALPHA_DECAY = 0.985;
const ALPHA_MIN = 0.004;
const MAX_VELOCITY = 18;

/**
 * A dependency-free, Obsidian-style force-directed graph of the knowledge base.
 * Entries are nodes (colored by kind); tags are hub nodes; an edge links an
 * entry to each of its tags so shared-tag entries cluster. Supports pan, zoom,
 * node drag, click-to-select, and hover highlighting.
 */
export function KnowledgeGraph({ entries, selectedId, search, onSelect }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 800, h: 600 });

  // Live simulation state kept in refs (mutated each frame, not React state).
  const nodesRef = useRef<Map<string, SimNode>>(new Map());
  const linksRef = useRef<SimLink[]>([]);
  const alphaRef = useRef(1);
  const rafRef = useRef<number | null>(null);

  const transformRef = useRef<Transform>({ x: 0, y: 0, k: 1 });
  const [, setTick] = useState(0); // bump to re-render from rAF

  const [hoverId, setHoverId] = useState<string | null>(null);

  // Interaction state (refs to avoid re-renders during drag/pan).
  const dragRef = useRef<{ id: string; moved: boolean } | null>(null);
  const panRef = useRef<{ startX: number; startY: number; ox: number; oy: number } | null>(null);
  const pointerRef = useRef<{ x: number; y: number } | null>(null);

  // Track container size.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      setSize({ w: Math.max(320, r.width), h: Math.max(240, r.height) });
    });
    ro.observe(el);
    const r = el.getBoundingClientRect();
    setSize({ w: Math.max(320, r.width), h: Math.max(240, r.height) });
    return () => ro.disconnect();
  }, []);

  // (Re)build the graph whenever entries change, preserving positions of nodes
  // that still exist so the layout doesn't jump on a refresh.
  useEffect(() => {
    const prev = nodesRef.current;
    const next = new Map<string, SimNode>();
    const links: SimLink[] = [];
    const cx = size.w / 2;
    const cy = size.h / 2;

    const ensure = (
      id: string,
      type: "entry" | "tag",
      label: string,
      seedAngle: number,
      entry?: KnowledgeEntry,
      tag?: string
    ): SimNode => {
      const existing = prev.get(id);
      if (existing) {
        existing.degree = 0;
        existing.label = label;
        existing.entry = entry ?? existing.entry;
        next.set(id, existing);
        return existing;
      }
      const radius = 120 + Math.random() * 80;
      const node: SimNode = {
        id,
        type,
        label,
        entry,
        tag,
        degree: 0,
        x: cx + Math.cos(seedAngle) * radius,
        y: cy + Math.sin(seedAngle) * radius,
        vx: 0,
        vy: 0,
      };
      next.set(id, node);
      return node;
    };

    entries.forEach((entry, i) => {
      const angle = (i / Math.max(1, entries.length)) * Math.PI * 2;
      const en = ensure(`entry:${entry.id}`, "entry", entry.title, angle, entry);
      en.entry = entry;
      for (const tag of entry.tags) {
        const tn = ensure(`tag:${tag}`, "tag", tag, angle + 0.3, undefined, tag);
        en.degree += 1;
        tn.degree += 1;
        links.push({ source: en.id, target: tn.id });
      }
    });

    nodesRef.current = next;
    linksRef.current = links;
    alphaRef.current = 1; // reheat
    setTick((n) => n + 1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entries, size.w, size.h]);

  // The animation loop: step the simulation until it cools, then idle.
  const step = useCallback(() => {
    const nodes = Array.from(nodesRef.current.values());
    const links = linksRef.current;
    const cx = size.w / 2;
    const cy = size.h / 2;
    let alpha = alphaRef.current;

    if (nodes.length > 0 && alpha > ALPHA_MIN) {
      // Repulsion (O(n^2), fine for a personal knowledge base).
      for (let i = 0; i < nodes.length; i++) {
        const a = nodes[i];
        for (let j = i + 1; j < nodes.length; j++) {
          const b = nodes[j];
          let dx = a.x - b.x;
          let dy = a.y - b.y;
          let d2 = dx * dx + dy * dy;
          if (d2 < 0.01) {
            dx = (Math.random() - 0.5) * 0.1;
            dy = (Math.random() - 0.5) * 0.1;
            d2 = 0.01;
          }
          const dist = Math.sqrt(d2);
          const f = (REPULSION * alpha) / d2;
          const fx = (dx / dist) * f;
          const fy = (dy / dist) * f;
          a.vx += fx;
          a.vy += fy;
          b.vx -= fx;
          b.vy -= fy;
        }
      }
      // Link springs.
      for (const link of links) {
        const s = nodesRef.current.get(link.source);
        const tn = nodesRef.current.get(link.target);
        if (!s || !tn) continue;
        const dx = tn.x - s.x;
        const dy = tn.y - s.y;
        const dist = Math.sqrt(dx * dx + dy * dy) || 0.01;
        const f = (dist - LINK_DISTANCE) * LINK_STRENGTH * alpha;
        const fx = (dx / dist) * f;
        const fy = (dy / dist) * f;
        s.vx += fx;
        s.vy += fy;
        tn.vx -= fx;
        tn.vy -= fy;
      }
      // Center gravity + integrate.
      for (const n of nodes) {
        n.vx += (cx - n.x) * CENTER_GRAVITY * alpha;
        n.vy += (cy - n.y) * CENTER_GRAVITY * alpha;
        n.vx *= DAMPING;
        n.vy *= DAMPING;
        n.vx = Math.max(-MAX_VELOCITY, Math.min(MAX_VELOCITY, n.vx));
        n.vy = Math.max(-MAX_VELOCITY, Math.min(MAX_VELOCITY, n.vy));
        if (n.fx != null && n.fy != null) {
          n.x = n.fx;
          n.y = n.fy;
          n.vx = 0;
          n.vy = 0;
        } else {
          n.x += n.vx;
          n.y += n.vy;
        }
      }
      alpha *= ALPHA_DECAY;
      alphaRef.current = alpha;
      setTick((n) => n + 1);
    }
    rafRef.current = requestAnimationFrame(step);
  }, [size.w, size.h]);

  useEffect(() => {
    rafRef.current = requestAnimationFrame(step);
    return () => {
      if (rafRef.current != null) cancelAnimationFrame(rafRef.current);
    };
  }, [step]);

  const reheat = () => {
    alphaRef.current = Math.max(alphaRef.current, 0.6);
  };

  // --- coordinate helpers ---------------------------------------------------
  const screenToWorld = (sx: number, sy: number) => {
    const tr = transformRef.current;
    return { x: (sx - tr.x) / tr.k, y: (sy - tr.y) / tr.k };
  };

  const localPoint = (e: React.PointerEvent | React.WheelEvent) => {
    const rect = containerRef.current?.getBoundingClientRect();
    return { x: e.clientX - (rect?.left ?? 0), y: e.clientY - (rect?.top ?? 0) };
  };

  // --- pointer interaction --------------------------------------------------
  const onPointerDownNode = (e: React.PointerEvent, node: SimNode) => {
    e.stopPropagation();
    (e.target as Element).setPointerCapture?.(e.pointerId);
    dragRef.current = { id: node.id, moved: false };
    const p = localPoint(e);
    pointerRef.current = p;
    const w = screenToWorld(p.x, p.y);
    node.fx = w.x;
    node.fy = w.y;
    reheat();
  };

  const onPointerDownBg = (e: React.PointerEvent) => {
    const p = localPoint(e);
    const tr = transformRef.current;
    panRef.current = { startX: p.x, startY: p.y, ox: tr.x, oy: tr.y };
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const p = localPoint(e);
    if (dragRef.current) {
      const node = nodesRef.current.get(dragRef.current.id);
      if (node) {
        const w = screenToWorld(p.x, p.y);
        node.fx = w.x;
        node.fy = w.y;
        dragRef.current.moved = true;
        reheat();
      }
    } else if (panRef.current) {
      transformRef.current = {
        ...transformRef.current,
        x: panRef.current.ox + (p.x - panRef.current.startX),
        y: panRef.current.oy + (p.y - panRef.current.startY),
      };
      setTick((n) => n + 1);
    }
  };

  const onPointerUp = (_e: React.PointerEvent) => {
    if (dragRef.current) {
      const node = nodesRef.current.get(dragRef.current.id);
      const wasClick = !dragRef.current.moved;
      if (node) {
        // Release the pin so the node rejoins the simulation.
        node.fx = null;
        node.fy = null;
        if (wasClick && node.type === "entry" && node.entry) {
          onSelect(node.entry);
        }
      }
      dragRef.current = null;
    }
    panRef.current = null;
  };

  const onWheel = (e: React.WheelEvent) => {
    e.preventDefault();
    const p = localPoint(e);
    const tr = transformRef.current;
    const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
    const k = Math.max(0.25, Math.min(3, tr.k * factor));
    // Zoom around the cursor.
    const wx = (p.x - tr.x) / tr.k;
    const wy = (p.y - tr.y) / tr.k;
    transformRef.current = { k, x: p.x - wx * k, y: p.y - wy * k };
    setTick((n) => n + 1);
  };

  const resetView = () => {
    transformRef.current = { x: 0, y: 0, k: 1 };
    reheat();
    setTick((n) => n + 1);
  };

  // --- search highlighting --------------------------------------------------
  const q = search.trim().toLowerCase();
  const matchIds = useMemo(() => {
    if (!q) return null;
    const ids = new Set<string>();
    for (const node of nodesRef.current.values()) {
      if (node.type === "entry" && node.entry) {
        const e = node.entry;
        if (
          e.title.toLowerCase().includes(q) ||
          e.body.toLowerCase().includes(q) ||
          e.tags.some((tg) => tg.toLowerCase().includes(q))
        ) {
          ids.add(node.id);
        }
      } else if (node.type === "tag" && node.tag?.toLowerCase().includes(q)) {
        ids.add(node.id);
      }
    }
    return ids;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q, entries]);

  // Neighbors of the hovered node, for highlight.
  const neighborIds = useMemo(() => {
    if (!hoverId) return null;
    const set = new Set<string>([hoverId]);
    for (const l of linksRef.current) {
      if (l.source === hoverId) set.add(l.target);
      if (l.target === hoverId) set.add(l.source);
    }
    return set;
  }, [hoverId]);

  const tr = transformRef.current;
  const nodes = Array.from(nodesRef.current.values());
  const links = linksRef.current;

  const nodeRadius = (n: SimNode) =>
    n.type === "entry" ? 7 + Math.min(6, n.degree) : 4 + Math.min(5, n.degree * 0.8);

  const isDimmed = (id: string): boolean => {
    if (matchIds && !matchIds.has(id)) return true;
    if (neighborIds && !neighborIds.has(id)) return true;
    return false;
  };

  return (
    <div ref={containerRef} className="relative w-full h-full overflow-hidden">
      {/* Controls */}
      <div className="absolute top-3 right-3 z-10 flex items-center gap-1.5">
        <button
          onClick={resetView}
          className="px-2.5 py-1 text-xs rounded-md bg-white/90 border border-border-theme text-text-secondary hover:text-text-base hover:bg-white transition-colors shadow-sm"
          title="Reset view"
        >
          {Math.round(tr.k * 100)}%
        </button>
      </div>

      <svg
        width={size.w}
        height={size.h}
        className="block touch-none select-none cursor-grab active:cursor-grabbing"
        onPointerDown={onPointerDownBg}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerLeave={onPointerUp}
        onWheel={onWheel}
      >
        <g transform={`translate(${tr.x},${tr.y}) scale(${tr.k})`}>
          {/* Links */}
          {links.map((l, i) => {
            const s = nodesRef.current.get(l.source);
            const t = nodesRef.current.get(l.target);
            if (!s || !t) return null;
            const dim = isDimmed(s.id) && isDimmed(t.id);
            return (
              <line
                key={i}
                x1={s.x}
                y1={s.y}
                x2={t.x}
                y2={t.y}
                stroke={dim ? "#e5e7eb" : "#cbd5e1"}
                strokeWidth={1 / tr.k}
                opacity={dim ? 0.35 : 0.8}
              />
            );
          })}
          {/* Nodes */}
          {nodes.map((n) => {
            const r = nodeRadius(n);
            const dim = isDimmed(n.id);
            const selected = n.entry != null && n.entry.id === selectedId;
            const fill = n.type === "entry" ? kindColor(n.entry?.kind ?? "note") : TAG_COLOR;
            const showLabel = tr.k > 0.75 || n.type === "entry";
            return (
              <g
                key={n.id}
                transform={`translate(${n.x},${n.y})`}
                opacity={dim ? 0.28 : 1}
                style={{ cursor: n.type === "entry" ? "pointer" : "default" }}
                onPointerDown={(e) => onPointerDownNode(e, n)}
                onPointerEnter={() => setHoverId(n.id)}
                onPointerLeave={() => setHoverId((h) => (h === n.id ? null : h))}
              >
                <circle
                  r={r}
                  fill={n.type === "tag" ? "#fff" : fill}
                  stroke={n.type === "tag" ? TAG_COLOR : selected ? "#111827" : "#fff"}
                  strokeWidth={(selected ? 3 : n.type === "tag" ? 1.5 : 1.5) / tr.k}
                />
                {selected && (
                  <circle r={r + 4 / tr.k} fill="none" stroke={fill} strokeWidth={1.5 / tr.k} opacity={0.5} />
                )}
                {showLabel && (
                  <text
                    x={0}
                    y={r + 11 / tr.k}
                    textAnchor="middle"
                    fontSize={11 / tr.k}
                    fill={n.type === "tag" ? "#94a3b8" : "#374151"}
                    fontWeight={n.type === "entry" ? 500 : 400}
                    style={{ pointerEvents: "none" }}
                  >
                    {n.type === "tag" ? `#${n.label}` : truncate(n.label, 24)}
                  </text>
                )}
              </g>
            );
          })}
        </g>
      </svg>
    </div>
  );
}

function truncate(s: string, max: number): string {
  return s.length <= max ? s : s.slice(0, max - 1) + "…";
}
