import { label } from './execution';
import type { OurRunState, RunProjection } from './model';
import { activeNodes } from './parallel';

export type RunControl = 'pause' | 'stop' | 'resume';
export type ProjectedRun = { projection?: RunProjection | null };

/** A missing projection is unknown, including on an older Host. Never grant a
 * control by independently interpreting its durable status in the browser. */
export const runPhase = (run: ProjectedRun) => run.projection?.phase ?? 'unavailable';
export const runNeedsAttention = (run: ProjectedRun) => run.projection?.needs_attention ?? true;
export const runPhaseLabel = (phase: string) => ({
  waiting_resume: '等待接续', waiting_call: '等待调用结果',
  budget_stopped: '预算已耗尽',
  stopping: '正在停止…', pausing: '正在暂停…', unavailable: '运行状态不可用',
  uncertain: '待核查', planned: '已计划', missed_busy: '忙碌错过', missed_downtime: '停机错过',
}[phase] ?? label(phase));

export const runControlLabels: Record<RunControl, string> = { pause: '暂停', stop: '停止', resume: '继续' };

/** A cursor locates a retained invocation; only the Host observation says it
 * still has an executor. Completed node results remain historical facts even
 * when that current observation is missing. A null state is a pure definition. */
export function runNodePresentation(state: OurRunState | null | undefined, node: string, projection?: RunProjection | null) {
  const atCursor = activeNodes(state).some(cursor => cursor.node === node);
  const phase = runPhase({ projection });
  const live = atCursor && projection?.active === true && ['running', 'pausing', 'stopping'].includes(phase);
  if (atCursor) {
    const status = phase === 'running' && !live ? 'unavailable' : phase;
    return { status, label: runPhaseLabel(status), summaryLabel: runPhaseLabel(status), atCursor, live };
  }
  const result = state?.nodes?.[node];
  if (result) {
    const status = result.exit_status === 'interrupted' ? 'interrupted' : result.submitted ? 'completed' : 'failed';
    const text = label(result.exit_status || status);
    return { status, label: text, summaryLabel: status === 'completed' ? '已提交' : text, atCursor, live };
  }
  if (state?.skipped?.includes(node)) return { status: 'skipped', label: '未选中', summaryLabel: '未选中', atCursor, live };
  if (state !== null && !projection) return { status: 'unavailable', label: runPhaseLabel('unavailable'), summaryLabel: runPhaseLabel('unavailable'), atCursor, live };
  return { status: 'pending', label: '尚未执行', summaryLabel: '未执行', atCursor, live };
}
