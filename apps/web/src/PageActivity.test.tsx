/** @vitest-environment jsdom */
import { act, StrictMode, useEffect, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PageActivity } from './PageActivity';

describe('visited page lifecycle', () => {
  let root: Root;
  let container: HTMLDivElement;
  const resume = vi.fn();
  const suspend = vi.fn();
  function Page() {
    const [count, setCount] = useState(0);
    useEffect(() => { resume(); return suspend; }, []);
    return <button onClick={() => setCount(previous => previous + 1)}>{count}</button>;
  }
  const draw = async (active: boolean) => act(async () => root.render(
    <StrictMode><PageActivity active={active}><Page /></PageActivity></StrictMode>,
  ));
  beforeEach(() => {
    vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true);
    resume.mockReset(); suspend.mockReset();
    container = document.createElement('div'); document.body.append(container); root = createRoot(container);
  });
  afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });

  it('does not mount unvisited pages, preserves their DOM and state, and suspends hidden effects', async () => {
    await draw(false);
    expect(container.querySelector('button')).toBeNull();
    expect(resume).not.toHaveBeenCalled();
    await draw(true);
    const button = container.querySelector('button')!;
    await act(async () => button.click());
    expect(button.textContent).toBe('1');
    const resumed = resume.mock.calls.length;
    const suspended = suspend.mock.calls.length;
    await draw(false);
    expect(container.querySelector('button')).toBe(button);
    expect(button.style.display).toBe('none');
    expect(suspend).toHaveBeenCalledTimes(suspended + 1);
    await draw(false);
    expect(resume).toHaveBeenCalledTimes(resumed);
    await draw(true);
    expect(container.querySelector('button')).toBe(button);
    expect(button.style.display).toBe('');
    expect(button.textContent).toBe('1');
    expect(resume.mock.calls.length - suspend.mock.calls.length).toBe(1);
  });
});
