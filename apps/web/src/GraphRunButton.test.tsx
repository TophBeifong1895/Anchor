/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { GraphRunButton, graphRunAction } from './GraphRunButton';
import type { GraphSummary } from './model';

describe('Graph run entry', () => {
  let container: HTMLDivElement;
  let root: Root;
  const onClick = vi.fn(async () => {});
  const render = async (graph?: GraphSummary, dirty = false) => {
    await act(async () => root.render(<GraphRunButton graph={graph} name="g" dirty={dirty} busy={false} onClick={onClick} />));
    return container.querySelector('button')!;
  };
  beforeEach(() => {
    vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true); onClick.mockClear();
    container = document.createElement('div'); root = createRoot(container);
  });
  afterEach(async () => { await act(async () => root.unmount()); vi.unstubAllGlobals(); });

  it.each([
    { graph: 'g', running: 'run-1' },
    { graph: 'g', running: null, active_runs: ['run-1', 'run-2'] },
  ])('opens an existing Run from either activity projection', async graph => {
    expect(graphRunAction(graph)).toEqual({ kind: 'view', run: 'run-1' });
    const button = await render(graph, true);
    expect(button.textContent).toBe('查看运行'); expect(button.disabled).toBe(false);
    await act(async () => button.click()); expect(onClick).toHaveBeenCalledOnce();
  });

  it('sends listener-bound Graphs to their message sessions even when instances are active', async () => {
    const graph: GraphSummary = { graph: 'g', running: 'run-1', active_runs: ['run-1'], listener: { platform: 'wecom', status: 'authenticated' } };
    expect(graphRunAction(graph)).toEqual({ kind: 'sessions' });
    const button = await render(graph);
    expect(button.textContent).toBe('查看会话');
    expect(button.title).toContain('消息会唤醒此工作流');
  });

  it('only offers a new Run for an inactive Graph and requires a saved draft', async () => {
    const graph = { graph: 'g', running: null, active_runs: [] };
    expect(graphRunAction(graph)).toEqual({ kind: 'start' });
    const button = await render(graph, true);
    expect(button.textContent).toBe('运行工作流'); expect(button.disabled).toBe(true);
    await act(async () => button.click()); expect(onClick).not.toHaveBeenCalled();
    expect((await render(graph)).disabled).toBe(false);
  });
});
