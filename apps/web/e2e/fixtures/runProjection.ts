import type { RunProjection } from '../../src/model';

/** Explicit HTTP fixture facts, never a browser-side authorization fallback. */
export const runProjectionFixture = (phase: RunProjection['phase'], overrides: Partial<RunProjection> = {}): RunProjection => ({
  phase, active: false, reason: null, updated: null,
  actions: { pause: false, stop: false, resume: false, abandon: false }, ...overrides,
});
