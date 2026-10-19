/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RunDetailPage } from './RunDetailPage';
import type { OurGraph, OurRunDetail, RunProjection } from './model';

const canvas = vi.hoisted(() => ({ render: vi.fn() }));
vi.mock('./WorkflowCanvas', () => ({ WorkflowCanvas: (props: unknown) => { canvas.render(props); return <div data-testid="canvas" />; } }));
vi.mock('./RunInspector', () => ({ RunInspector: () => null }));

const frozen: OurGraph = { entry: 'module/original', nodes: [{ id: 'module/original', agent: 'role' }], edges: [] };
const projection = (phase: RunProjection['phase'], extra: Partial<RunProjection> = {}): RunProjection => ({
  phase, active: false, reason: null, updated: null,
  actions: { stop: true, resume: true, abandon: true }, ...extra,
});
const detailFor = (view: RunProjection): OurRunDetail => ({
  run: 'original-run', graph: 'original-graph', definition: frozen, projection: view, nodes: ['module/original'], traces: {},
  state: { objective: 'frozen objective', started: '', updated: '', cursor: null,
    nodes: {}, passes: {}, decided: {}, executed: [], skipped: [], error: '' },
});

describe('Run-owned canvas and shared operation projection', () => {
  let element: HTMLDivElement; let root: Root;
  const control = vi.fn(async () => {});
  const render = async (detail: OurRunDetail, name = 'current-editor-graph') => act(async () => root.render(
    <RunDetailPage name={name} graphs={[]} dirty busy={false} notice={null} run={detail.run} node="" targetPath="" detail={detail}
      onPilot={() => {}} selectGraph={() => true} trigger={async () => {}} onTimeline={() => {}}
      navigateRun={async () => {}} controlRun={control} deleteRun={async () => {}} onNode={() => {}} />,
  ));
  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    element = document.createElement('div'); document.body.appendChild(element); root = createRoot(element);
    canvas.render.mockClear(); control.mockClear();
  });
  afterEach(async () => { await act(async () => root.unmount()); element.remove(); });

  it('renders the expanded frozen topology independently of the selected editor Graph', async () => {
    const detail = detailFor(projection('settled', { outcome: 'done' }));
    await render(detail);
    expect(canvas.render.mock.lastCall?.[0]).toMatchObject({ graph: frozen, name: 'original-graph', mode: 'run', projection: detail.projection });
    await render(detail, 'another-editor-graph');
    expect(canvas.render.mock.lastCall?.[0].graph.nodes).toEqual([{ id: 'module/original', agent: 'role' }]);
  });

  it('reports missing frozen topology without rendering a live authoring fallback', async () => {
    await render({ ...detailFor(projection('settled', { outcome: 'done' })), definition: undefined });
    expect(element.querySelector('[data-testid="canvas"]')).toBeNull();
    expect(element.textContent).toContain('该运行缺少冻结拓扑');
  });

  it.each([
    [projection('waiting', { waiting_for: 'child' }), '等待子任务'],
    [projection('executing'), '等待接续'],
    [projection('waiting', { waiting_for: 'effect', reason: '工具结果未记录' }), '等待核查效果：工具结果未记录'],
  ] as const)('exposes the server-projected actions for %s', async (view, label) => {
    // Intentionally independent action facts: the component never derives authorization.
    await render(detailFor(view));
    const buttons = [...element.querySelectorAll<HTMLButtonElement>('.run-controls button')];
    expect(buttons.map(button => button.textContent)).toEqual(['停止', '继续', '放弃']);
    await act(async () => buttons[0].click());
    await act(async () => buttons[1].click());
    await act(async () => buttons[2].click());
    expect(control.mock.calls).toEqual([['stop'], ['resume'], ['abandon']]);
    expect(element.textContent).toContain(label);
  });

  it('obeys denied actions without deriving resume from durable status', async () => {
    const detail = detailFor(projection('waiting', { waiting_for: 'human', reason: 'paused' }));
    detail.projection!.actions = { stop: false, resume: false, abandon: false };
    await render(detail);
    expect(element.querySelectorAll('.run-controls button')).toHaveLength(0);
    await render({ ...detail, projection: undefined });
    expect(element.querySelectorAll('.run-controls button')).toHaveLength(0);
    expect(element.textContent).toContain('运行状态不可用');
  });

  it('uses the projection for cursor and control hints despite conflicting legacy fields', async () => {
    const detail = detailFor(projection('executing', { active: true }));
    detail.active = true; detail.state.reason = 'asked';
    detail.state.cursor = { node: 'module/original', pass: 1, dir: '' };
    detail.projection = projection('executing');
    await render(detail);
    expect(element.querySelector('[aria-label="活动节点"]')?.textContent).toContain('等待接续');
    expect(element.textContent).not.toContain('请求已接收');
    expect(element.textContent).not.toContain('已按请求停止');
    await render({ ...detail, projection: undefined });
    expect(element.querySelector('[aria-label="活动节点"]')?.textContent).toContain('运行状态不可用');
    expect(element.querySelector('[aria-label="活动节点"]')?.textContent).not.toContain('正在执行');
  });

  it('shows recovery guidance only when recovery is the projected phase', async () => {
    const detail = detailFor(projection('waiting', { waiting_for: 'human', reason: 'paused' }));
    detail.state.error = 'Agent 上次工具调用的结果未记录';
    await render(detail);
    expect(element.textContent).not.toContain('Agent 上次工具调用的结果未记录');
    detail.projection = projection('waiting', { waiting_for: 'effect', reason: 'Agent 上次工具调用的结果未记录' });
    await render(detail);
    expect(element.textContent).toContain('Agent 上次工具调用的结果未记录');
  });
});
