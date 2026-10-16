import type { RunProjection } from '../../src/model';

/** Explicit HTTP fixture facts, never a browser-side authorization fallback. */
export const runProjectionFixture = (phase: string, overrides: Partial<RunProjection> = {}): RunProjection => ({
  phase, active: false, control_requested: null, terminal: false, quiescent: false,
  needs_attention: false, actions: { pause: false, stop: false, resume: false }, ...overrides,
});
