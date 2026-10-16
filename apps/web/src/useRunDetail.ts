import { useEffect, useState } from 'react';
import { api } from './api';
import type { OurRun, OurRunDetail } from './model';

/** Run detail belongs to the inspection page, and follows its list refreshes. */
export function useRunDetail(run: string, enabled: boolean, runs: OurRun[]) {
  const [loaded, setLoaded] = useState<{ run: string; detail: OurRunDetail } | null>(null);
  useEffect(() => {
    if (!enabled || !run) return;
    const controller = new AbortController();
    void api<OurRunDetail>(`/runs/${encodeURIComponent(run)}`, 'GET', undefined, controller.signal)
      .then(detail => { if (!controller.signal.aborted) setLoaded({ run, detail }); })
      .catch(() => { if (!controller.signal.aborted) setLoaded(null); });
    return () => controller.abort();
  }, [run, enabled, runs]);
  return loaded?.run === run ? loaded.detail : null;
}
