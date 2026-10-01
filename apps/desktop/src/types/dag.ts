// DAG execution types for plan_execute tool

export type DagNodeStatus = 'pending' | 'running' | 'done' | 'failed';

export interface DagNode {
  id: string;
  goal: string;
  phase?: string;
  role?: string;
  status: DagNodeStatus;
  dependsOn: string[];
  startedAt?: number;
  completedAt?: number;
  duration?: number;
  summary?: string;
  error?: string;
}

export interface DagExecution {
  executionId: string;
  title: string;
  nodes: DagNode[];
  createdAt: number;
  updatedAt: number;
}

export interface DagStatusUpdate {
  executionId: string;
  nodeId: string;
  status: DagNodeStatus;
  summary?: string;
  error?: string;
  duration?: number;
}
