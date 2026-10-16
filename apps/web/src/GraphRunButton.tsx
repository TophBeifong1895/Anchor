import { Activity, Play, Radio } from 'lucide-react';
import type { GraphSummary } from './model';

/** Channel assistants are admitted by messages; an existing Run is opened instead of duplicated. */
export const graphRunAction = (graph?: GraphSummary): { kind: 'channels' } | { kind: 'view'; run: string } | { kind: 'start' } => {
  if (graph?.listener) return { kind: 'channels' };
  const run = graph?.active_runs?.[0] || graph?.running;
  return run ? { kind: 'view', run } : { kind: 'start' };
};

export function GraphRunButton({ graph, name, dirty, busy, onClick }: {
  graph?: GraphSummary; name: string; dirty: boolean; busy: boolean; onClick: () => Promise<void>;
}) {
  const action = graphRunAction(graph);
  const title = action.kind === 'channels' ? '在渠道中查看和管理常驻助手，消息会接纳对应实例'
    : action.kind === 'view' ? '查看此工作流正在执行的实例'
      : dirty ? '请先保存工作流' : '运行已保存的工作流';
  return <button className="primary" title={title} onClick={() => void onClick()}
    disabled={busy || !name || action.kind === 'start' && dirty}>
    {action.kind === 'channels' ? <Radio size={14} /> : action.kind === 'view' ? <Activity size={14} /> : <Play size={14} fill="currentColor" />}
    {action.kind === 'channels' ? '查看渠道' : action.kind === 'view' ? '查看运行' : '运行工作流'}
  </button>;
}
