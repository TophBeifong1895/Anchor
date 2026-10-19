import type { RunProjection } from './model';
import { runControlLabels, type RunControl } from './runPresentation';

const titles: Record<RunControl, string> = {
  abandon: '放弃本次运行；已发生的外部效果不会撤回',
  stop: '停止本次运行及等待模式创建的目标；已接纳的启动后继续调用不受影响',
  resume: '按原运行上下文继续，必要时先核查中断现场',
};

/** All Run entry points consume the Host's operation projection. Errors and
 * refresh belong to the caller's existing operation lifecycle. */
export function RunControls({ projection, busy, onControl }: {
  projection?: RunProjection; busy: boolean; onControl: (operation: RunControl) => void;
}) {
  if (!projection) return null;
  return <span className="run-controls">
    {(['stop', 'resume', 'abandon'] as const).filter(operation =>
      projection.actions[operation]).map(operation =>
      <button key={operation} type="button" className={operation === 'stop' ? 'danger-link' : undefined}
        title={titles[operation]} disabled={busy || !projection.actions[operation]}
        onClick={() => onControl(operation)}>{runControlLabels[operation]}</button>)}
  </span>;
}
