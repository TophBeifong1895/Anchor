/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api } from './api';
import { useWorkbenchData, type WorkbenchView } from './useWorkbenchData';

vi.mock('./api', () => ({ api: vi.fn() }));
describe('workbench page subscriptions', () => {
  let root: Root;
  let container: HTMLDivElement;
  let state: ReturnType<typeof useWorkbenchData>;
  const Harness = ({ view }: { view: WorkbenchView }) => { state = useWorkbenchData(view); return null; };
  const render = async (view: WorkbenchView) => act(async () => root.render(<Harness view={view} />));
  beforeEach(() => {
    vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true); vi.useFakeTimers();
    container = document.createElement('div'); root = createRoot(container);
    vi.mocked(api).mockReset().mockImplementation(async path => path === '/graphs' ? { graphs: [{ graph: 'g' }] }
      : path === '/runs' ? { runs: [] } : { runs: [], scheduled: [], schedules: [] });
  });
  afterEach(async () => { await act(async () => root.unmount()); vi.useRealTimers(); vi.unstubAllGlobals(); });

  it('suspends workbench polling on Pilot and restores each page subscription immediately', async () => {
    await render('graph');
    expect(api).toHaveBeenCalledTimes(2);
    expect(vi.mocked(api).mock.calls.map(([path]) => path)).toEqual(['/graphs', '/runs']);
    await act(async () => vi.advanceTimersByTimeAsync(3000));
    expect(api).toHaveBeenCalledTimes(4);
    await render('pilot');
    await act(async () => vi.advanceTimersByTimeAsync(9000));
    expect(api).toHaveBeenCalledTimes(4);
    await render('runs');
    expect(vi.mocked(api).mock.calls.slice(-3).map(([path]) => path)).toEqual(['/graphs', '/runs', expect.stringContaining('/timeline?')]);
    await render('graph');
    expect(vi.mocked(api).mock.calls.slice(-2).map(([path]) => path)).toEqual(['/graphs', '/runs']);
  });

  it('checks first-visit API authorization on Pilot without starting Run or timeline polling', async () => {
    await render('pilot');
    expect(vi.mocked(api).mock.calls.map(([path]) => path)).toEqual(['/graphs']);
    await act(async () => vi.advanceTimersByTimeAsync(12000));
    expect(api).toHaveBeenCalledTimes(1);
  });

  it('keeps unchanged projection identities but immediately publishes changed Host facts', async () => {
    await render('graph');
    const graphs = state.graphs, runs = state.runs;
    await act(async () => vi.advanceTimersByTimeAsync(3000));
    expect(state.graphs).toBe(graphs);
    expect(state.runs).toBe(runs);
    vi.mocked(api).mockImplementation(async path => path === '/graphs'
      ? { graphs: [{ graph: 'g', listener: { status: 'unavailable' } }] }
      : { runs: [{ run: 'new', running: true }] });
    await act(async () => vi.advanceTimersByTimeAsync(3000));
    expect(state.graphs).not.toBe(graphs);
    expect(state.graphs[0].listener?.status).toBe('unavailable');
    expect(state.runs).not.toBe(runs);
    expect(state.runs[0].run).toBe('new');
  });

  it('ignores a projection received after its page was left', async () => {
    await render('graph');
    let release!: (value: unknown) => void;
    vi.mocked(api).mockImplementation(path => path === '/graphs' ? new Promise(resolve => { release = resolve; })
      : Promise.resolve({ runs: [] }));
    await act(async () => vi.advanceTimersByTimeAsync(3000));
    const signal = vi.mocked(api).mock.calls.at(-2)![3]!;
    await render('pilot'); expect(signal.aborted).toBe(true);
    await act(async () => release({ graphs: [{ graph: 'late' }] }));
    expect(state.graphs.map(item => item.graph)).toEqual(['g']);
  });
});
