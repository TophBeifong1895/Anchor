import { describe, expect, it } from 'vitest';
import type { RunProjection } from './model';
import { runNeedsAttention, runPhaseLabel } from './runPresentation';

const none = { pause: false, stop: false, resume: false, abandon: false };
const projection = (value: Partial<RunProjection>): RunProjection => ({
  phase: 'waiting', waiting_for: 'human', reason: 'stopped', active: false, actions: none, updated: null, ...value,
});

describe('a Run taken over by a later Run of its conversation', () => {
  it('reads as replaced, whatever it was waiting on, and needs no attention', () => {
    for (const value of [projection({ superseded: true }), projection({ phase: 'executing', superseded: true })]) {
      expect(runPhaseLabel(value)).toBe('已由新实例接替');
      expect(runNeedsAttention({ projection: value })).toBe(false);
    }
  });

  it('keeps its settled outcome next to the takeover', () => {
    expect(runPhaseLabel(projection({ phase: 'settled', outcome: 'aborted', reason: 'x', superseded: true })))
      .toBe('已放弃，已由新实例接替');
    expect(runPhaseLabel(projection({ phase: 'settled', outcome: 'done', reason: null, superseded: true }))).toBe('已完成');
  });

  it('is still shown live while a worker runs it', () => {
    expect(runPhaseLabel(projection({ phase: 'executing', active: true, superseded: true }))).toBe('执行中');
  });

  it('leaves a Run that was not taken over unchanged', () => {
    expect(runPhaseLabel(projection({}))).toBe('等待你处理：stopped');
    expect(runNeedsAttention({ projection: projection({}) })).toBe(true);
  });
});
