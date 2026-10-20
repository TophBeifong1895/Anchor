import { Activity, useState, type ReactNode } from 'react';

export function PageActivity({ active, children }: { active: boolean; children: ReactNode }) {
  const [visited, setVisited] = useState(active);
  if (active && !visited) setVisited(true);
  return visited || active ? <Activity mode={active ? 'visible' : 'hidden'}>{children}</Activity> : null;
}
