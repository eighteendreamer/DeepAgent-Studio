import { useState, useMemo } from "react";
import { ChevronDown, ChevronRight, Clock, CheckCircle, Loader2, XCircle } from "lucide-react";
import type { DagExecution, DagNode } from "../../types/dag";

interface DagExecutionRowProps {
  execution: DagExecution;
}

/**
 * DAG execution display component that matches the style of ProcessToolRow.
 * Shows plan_execute tool results with collapsible phase-grouped nodes.
 */
export function DagExecutionRow({ execution }: DagExecutionRowProps) {
  const [open, setOpen] = useState(false);

  // Group nodes by phase
  const nodesByPhase = useMemo(() => {
    const groups: Record<string, DagNode[]> = {};
    execution.nodes.forEach(node => {
      const phase = node.phase || 'Default';
      if (!groups[phase]) groups[phase] = [];
      groups[phase].push(node);
    });
    return groups;
  }, [execution.nodes]);

  // Calculate overall stats
  const stats = useMemo(() => {
    const total = execution.nodes.length;
    const done = execution.nodes.filter(n => n.status === 'done').length;
    const running = execution.nodes.filter(n => n.status === 'running').length;
    const failed = execution.nodes.filter(n => n.status === 'failed').length;
    const pending = total - done - running - failed;
    return { total, done, running, failed, pending };
  }, [execution.nodes]);

  const isRunning = stats.running > 0;
  const allDone = stats.done === stats.total;
  const hasFailed = stats.failed > 0;

  // Status icon and styling
  const statusIcon = hasFailed ? XCircle : isRunning ? Loader2 : allDone ? CheckCircle : Clock;
  const statusClass = hasFailed
    ? "text-red-600"
    : isRunning
    ? "text-blue-600"
    : allDone
    ? "text-green-600"
    : "text-gray-400";

  const statusLabel = hasFailed
    ? "Failed"
    : isRunning
    ? "Running"
    : allDone
    ? "Done"
    : "Pending";

  const StatusIcon = statusIcon;
  const chevronClass = "shrink-0 text-text-secondary opacity-45 transition group-hover/tool:opacity-75";

  return (
    <div className="min-w-0">
      {/* Main Row - matches ProcessToolRow style */}
      <button
        type="button"
        onClick={() => setOpen(value => !value)}
        className={`group/tool flex w-full items-center gap-2.5 rounded-lg px-3 py-2.5 text-left transition ${
          open ? "bg-gray-50/75" : "hover:bg-gray-50/40"
        }`}
      >
        {open ? (
          <ChevronDown className={chevronClass} size={16} />
        ) : (
          <ChevronRight className={chevronClass} size={16} />
        )}

        {/* Status Icon */}
        <StatusIcon
          size={16}
          className={`shrink-0 ${statusClass} ${isRunning ? 'animate-spin' : ''}`}
        />

        {/* Tool Name Badge */}
        <div className="shrink-0 rounded bg-blue-100 px-1.5 py-0.5 font-mono text-[11px] font-medium text-blue-900">
          plan_execute
        </div>

        {/* Status Badge */}
        <div className={`shrink-0 rounded px-1.5 py-0.5 text-[11px] font-medium ${
          hasFailed
            ? "bg-red-100 text-red-900"
            : isRunning
            ? "bg-blue-100 text-blue-900"
            : allDone
            ? "bg-green-100 text-green-900"
            : "bg-gray-100 text-gray-600"
        }`}>
          {statusLabel}
        </div>

        {/* Title & Summary */}
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium text-text-base">
            {execution.title}
          </div>
          <div className="truncate text-xs text-text-secondary">
            {stats.done}/{stats.total} nodes complete
            {stats.running > 0 && ` · ${stats.running} running`}
            {stats.failed > 0 && ` · ${stats.failed} failed`}
          </div>
        </div>
      </button>

      {/* Expanded Content */}
      {open && (
        <div className="ml-6 mt-2 space-y-4 border-l-2 border-gray-200 pl-4">
          {/* Progress Bar */}
          <div className="space-y-1">
            <div className="flex justify-between text-xs text-text-secondary">
              <span>Progress</span>
              <span>{Math.round((stats.done / stats.total) * 100)}%</span>
            </div>
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-gray-200">
              <div
                className="h-full bg-blue-500 transition-all duration-500"
                style={{ width: `${(stats.done / stats.total) * 100}%` }}
              />
            </div>
          </div>

          {/* Phases */}
          {Object.entries(nodesByPhase).map(([phase, nodes]) => (
            <PhaseSection key={phase} phase={phase} nodes={nodes} />
          ))}
        </div>
      )}
    </div>
  );
}

interface PhaseSectionProps {
  phase: string;
  nodes: DagNode[];
}

function PhaseSection({ phase, nodes }: PhaseSectionProps) {
  const doneCount = nodes.filter(n => n.status === 'done').length;
  const totalCount = nodes.length;

  return (
    <div className="space-y-2">
      {/* Phase Header */}
      <div className="flex items-center justify-between">
        <div className="text-xs font-semibold text-text-base">
          Phase: {phase}
        </div>
        <div className="rounded bg-gray-100 px-2 py-0.5 text-xs font-medium text-gray-700">
          {doneCount}/{totalCount}
        </div>
      </div>

      {/* Nodes */}
      <div className="space-y-1.5">
        {nodes.map(node => (
          <NodeRow key={node.id} node={node} />
        ))}
      </div>
    </div>
  );
}

interface NodeRowProps {
  node: DagNode;
}

function NodeRow({ node }: NodeRowProps) {
  const statusIcon = {
    pending: { icon: '⏸️', label: 'Pending', color: 'text-gray-500' },
    running: { icon: '⚙️', label: 'Running', color: 'text-blue-600' },
    done: { icon: '✅', label: 'Done', color: 'text-green-600' },
    failed: { icon: '❌', label: 'Failed', color: 'text-red-600' },
  }[node.status];

  return (
    <div className="flex items-start gap-2 rounded bg-gray-50/50 px-2.5 py-2 text-xs">
      <span className="text-base leading-none">{statusIcon.icon}</span>
      <div className="min-w-0 flex-1">
        {/* Node ID & Badge */}
        <div className="mb-1 flex items-center gap-1.5">
          <span className="font-mono text-xs text-text-secondary">{node.id}</span>
          <span className={`rounded px-1.5 py-0.5 text-[10px] font-medium ${
            node.status === 'done'
              ? 'bg-green-100 text-green-900'
              : node.status === 'running'
              ? 'bg-blue-100 text-blue-900'
              : node.status === 'failed'
              ? 'bg-red-100 text-red-900'
              : 'bg-gray-100 text-gray-600'
          }`}>
            {statusIcon.label}
          </span>
          {node.duration && (
            <span className="font-mono text-[10px] text-text-tertiary">
              {formatDuration(node.duration)}
            </span>
          )}
        </div>

        {/* Summary or Dependencies */}
        {node.summary && (
          <div className="text-xs text-text-secondary">
            └─ {node.summary}
          </div>
        )}
        {node.error && (
          <div className="text-xs text-red-600">
            └─ Error: {node.error}
          </div>
        )}
        {!node.summary && !node.error && node.status === 'pending' && node.dependsOn.length > 0 && (
          <div className="text-xs text-text-tertiary">
            └─ Depends on: {node.dependsOn.join(', ')}
          </div>
        )}
      </div>
    </div>
  );
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const seconds = (ms / 1000).toFixed(1);
  return `${seconds}s`;
}
