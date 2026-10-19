import { Activity, Play, Radio } from 'lucide-react';
import type { GraphSummary } from './model';

/** A listening Graph is admitted by messages: its current instance is opened instead of duplicated. */
export const graphRunAction = (graph?: GraphSummary): { kind: 'sessions' } | { kind: 'view'; run: string } | { kind: 'start' } => {
  if (graph?.listener) return { kind: 'sessions' };
  const run = graph?.active_runs?.[0] || graph?.running;
  return run ? { kind: 'view', run } : { kind: 'start' };
};

export function GraphRunButton({ graph, name, dirty, busy, onClick }: {
  graph?: GraphSummary; name: string; dirty: boolean; busy: boolean; onClick: () => Promise<void>;
}) {
  const action = graphRunAction(graph);
  const title = action.kind === 'sessions' ? '消息会唤醒此工作流；在此查看按对话分开的运行实例'
    : action.kind === 'view' ? '查看此工作流正在执行的实例'
      : dirty ? '请先保存工作流' : '运行已保存的工作流';
  return <button className="primary" title={title} onClick={() => void onClick()}
    disabled={busy || !name || action.kind === 'start' && dirty}>
    {action.kind === 'sessions' ? <Radio size={14} /> : action.kind === 'view' ? <Activity size={14} /> : <Play size={14} fill="currentColor" />}
    {action.kind === 'sessions' ? '查看会话' : action.kind === 'view' ? '查看运行' : '运行工作流'}
  </button>;
}
