import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from './api';
import type { GraphSummary, OurRun, TimelineData } from './model';

export type WorkbenchView = 'graph' | 'runs' | 'runDetail' | 'pilot' | 'plugins' | 'settings';
type ApiFetcher = <T>(path: string) => Promise<T>;

/** Timeline is an optional projection, requested only by its page. */
export async function loadWorkbenchData(apiCall: ApiFetcher, timelinePath?: string) {
  const [graphList, runList] = await Promise.all([
    apiCall<{ graphs: GraphSummary[] }>('/graphs'),
    apiCall<{ runs: OurRun[] }>('/runs'),
  ]);
  if (!timelinePath) return { graphs: graphList.graphs, runs: runList.runs };
  try {
    const timeline = await apiCall<TimelineData>(timelinePath);
    return { graphs: graphList.graphs, runs: runList.runs, timeline, timelineProblem: '' };
  } catch (error) {
    return { graphs: graphList.graphs, runs: runList.runs, timeline: null,
      timelineProblem: error instanceof Error ? error.message : String(error) };
  }
}

function timelinePath(page: number) {
  const cutoff = new Date();
  cutoff.setHours(0, 0, 0, 0); cutoff.setDate(cutoff.getDate() + 1 - page * 30);
  const before = `${cutoff.getFullYear()}-${String(cutoff.getMonth() + 1).padStart(2, '0')}-${String(cutoff.getDate()).padStart(2, '0')}`;
  return `/timeline?days=30&before=${before}`;
}

/** Owns disposable Host projections and their subscriptions; editor drafts live elsewhere. */
export function useWorkbenchData(view: WorkbenchView) {
  const [graphs, setGraphs] = useState<GraphSummary[]>([]);
  const [runs, setRuns] = useState<OurRun[]>([]);
  const [timeline, setTimeline] = useState<TimelineData | null>(null);
  const [timelineProblem, setTimelineProblem] = useState('');
  const [timelinePage, setTimelinePage] = useState(0);
  const [problem, setProblem] = useState('');
  const request = useRef<AbortController | null>(null);
  const version = useRef(0);
  const authenticated = useRef(false);
  const subscribed = view === 'graph' || view === 'runs' || view === 'runDetail';
  const page = view === 'runs' ? timelinePage : null;

  const refresh = useCallback(async () => {
    const current = ++version.current;
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    const read: ApiFetcher = path => api(path, 'GET', undefined, controller.signal);
    try {
      // A first visit to Pilot still verifies the Host's API-key requirement once.
      // After that the Pilot owns its session requests, without background workbench polling.
      const data = subscribed ? await loadWorkbenchData(read, page === null ? undefined : timelinePath(page))
        : { ...(await read<{ graphs: GraphSummary[] }>('/graphs')) };
      if (controller.signal.aborted || current !== version.current) return;
      authenticated.current = true;
      setGraphs(data.graphs);
      if ('runs' in data) setRuns(data.runs);
      if ('timeline' in data) { setTimeline(data.timeline ?? null); setTimelineProblem(data.timelineProblem ?? ''); }
      setProblem('');
    } catch (error) {
      if (controller.signal.aborted || current !== version.current) return;
      setGraphs(previous => previous.map(item => item.listener
        ? { ...item, listener: { ...item.listener, status: 'unavailable' } } : item));
      const message = (error as Error).message;
      setProblem(error instanceof TypeError || /fetch|network|ECONNREFUSED|proxy error/i.test(message)
        ? '后端 API 暂不可达，请检查 8077 服务及前端代理。' : `后端请求失败：${message}`);
    }
  }, [subscribed, page]);

  useEffect(() => {
    if (subscribed || !authenticated.current) void refresh();
    const timer = subscribed ? window.setInterval(() => void refresh(), 3000) : undefined;
    return () => { window.clearInterval(timer); version.current += 1; request.current?.abort(); };
  }, [refresh, subscribed]);

  const forgetGraph = (graph: string) => {
    // A list request started before deletion must not resurrect its row.
    version.current += 1; request.current?.abort();
    setGraphs(items => items.filter(item => item.graph !== graph));
    setRuns(items => items.filter(item => item.graph !== graph));
  };
  return { graphs, runs, timeline, timelineProblem, timelinePage, setTimelinePage, problem, refresh, forgetGraph };
}
