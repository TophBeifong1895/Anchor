/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RunControls } from './RunControls';
import type { RunProjection } from './model';

describe('Run controls', () => {
  let element: HTMLDivElement; let root: Root;
  const control = vi.fn();
  const render = async (projection?: RunProjection, busy = false) => act(async () => root.render(
    <RunControls projection={projection} busy={busy} onControl={control} />,
  ));
  const executing = (): RunProjection => ({
    phase: 'executing', active: true, reason: null, updated: null,
    actions: { pause: true, stop: true, resume: false, abandon: true },
  });
  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    element = document.createElement('div'); document.body.appendChild(element); root = createRoot(element);
    control.mockClear();
  });
  afterEach(async () => { await act(async () => root.unmount()); element.remove(); });

  it('offers pause and stop independently with their distinct boundary semantics', async () => {
    await render(executing());
    const buttons = [...element.querySelectorAll('button')];
    expect(buttons.map(button => button.textContent)).toEqual(['暂停', '停止', '放弃']);
    expect(buttons[0].title).toContain('当前节点完成后暂停');
    await act(async () => buttons[0].click());
    await act(async () => buttons[1].click());
    expect(control.mock.calls).toEqual([['pause'], ['stop']]);
  });

  it('exposes only abandonment while waiting for an unknown effect', async () => {
    await render({ phase: 'waiting', waiting_for: 'effect', active: false, reason: '工具结果未记录', updated: null,
      actions: { pause: false, stop: false, resume: false, abandon: true } });
    const buttons = [...element.querySelectorAll('button')];
    expect(buttons.map(button => button.textContent)).toEqual(['放弃']);
    expect(buttons[0].title).toContain('已发生的外部效果不会撤回');
    await act(async () => buttons[0].click());
    expect(control.mock.calls).toEqual([['abandon']]);
  });

  it('does not derive pause authorization from an executing worker', async () => {
    const projection = executing(); projection.actions.pause = false;
    await render(projection);
    expect([...element.querySelectorAll('button')].map(button => button.textContent)).toEqual(['停止', '放弃']);
    await render(undefined);
    expect(element.querySelectorAll('button')).toHaveLength(0);
  });

  it('blocks every projected operation while an action is pending', async () => {
    await render({ ...executing(), actions: { pause: true, stop: true, resume: true, abandon: true } }, true);
    const buttons = [...element.querySelectorAll('button')];
    expect(buttons).toHaveLength(4);
    expect(buttons.every(button => button.disabled)).toBe(true);
    await act(async () => buttons.forEach(button => button.click()));
    expect(control).not.toHaveBeenCalled();
  });
});
