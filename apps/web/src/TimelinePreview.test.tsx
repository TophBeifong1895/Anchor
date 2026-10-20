/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Timeline } from './Timeline';
import type { OurRun, OurRunDetail, RunProjection } from './model';

describe('Timeline Run failure details', () => {
  let element: HTMLDivElement; let root: Root;
  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    Object.defineProperty(HTMLDialogElement.prototype, 'showModal', { configurable: true, value() { this.open = true; } });
    Object.defineProperty(HTMLDialogElement.prototype, 'close', { configurable: true, value() { this.open = false; } });
    element = document.createElement('div'); document.body.appendChild(element); root = createRoot(element);
  });
  afterEach(async () => { await act(async () => root.unmount()); element.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

  it('shows the failed Run reason returned in the HTTP detail after opening its bar', async () => {
    const reason = '节点 assistant 未提交结果：工具连接已断开';
    const projection: RunProjection = { phase: 'settled', outcome: 'failed', active: false, reason, updated: null,
      actions: { pause: false, stop: false, resume: false, abandon: false } };
    const started = new Date(Date.now() - 60_000).toISOString();
    const run: OurRun = { run: 'failed-run', graph: 'failure-graph', running: false, started,
      updated: new Date().toISOString(), executed: [], objective: '检查失败原因', projection };
    const detail: OurRunDetail = { run: run.run, graph: run.graph, projection, nodes: [], traces: {},
      state: { objective: run.objective, input: { topic: 'failure evidence' }, started, updated: run.updated,
        cursor: null, nodes: {}, passes: {}, decided: {}, executed: [], skipped: [], error: reason, recovery: [] } };
    const fetchMock = vi.fn(async (_path: string) => ({ ok: true, status: 200, json: async () => detail }) as Response);
    vi.stubGlobal('fetch', fetchMock);
    await act(async () => root.render(<Timeline data={{ runs: [run], scheduled: [], schedules: [] }} graphs={[run.graph]}
      page={0} onPage={() => {}} onSelect={() => {}} onRefresh={() => {}} />));

    await act(async () => element.querySelector<HTMLButtonElement>('[data-run-id="failed-run"]')!.click());

    expect(fetchMock.mock.calls[0]?.[0]).toBe('/runs/failed-run');
    const preview = element.querySelector('.timeline-drawer')!;
    expect(preview.querySelector('[role="alert"]')?.textContent).toBe(reason);
    expect(preview.querySelector('.board-badge')?.textContent).toBe('失败');
    expect(preview.querySelector('.preview-input')?.textContent).toContain('failure evidence');
  });
});
