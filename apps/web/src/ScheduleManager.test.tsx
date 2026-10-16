/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import { ScheduleManager } from './ScheduleManager';

it('can submit the only Graph when the catalog arrives after the form opens', async () => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  Object.defineProperty(HTMLDialogElement.prototype, 'showModal', { configurable: true, value() { this.open = true; } });
  Object.defineProperty(HTMLDialogElement.prototype, 'close', { configurable: true, value() { this.open = false; } });
  const container = document.createElement('div'); document.body.appendChild(container);
  const root = createRoot(container);
  const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 200, json: async () => ({ schedule: { id: 'saved' } }) });
  vi.stubGlobal('fetch', fetchMock);
  const props = { schedules: [], refresh: vi.fn(), close: vi.fn() };
  try {
    await act(async () => root.render(<ScheduleManager {...props} graphs={[]} />));
    expect(container.querySelector<HTMLButtonElement>('button[type="submit"]')!.disabled).toBe(true);
    await act(async () => root.render(<ScheduleManager {...props} graphs={['only']} />));
    expect(container.querySelector<HTMLButtonElement>('button[type="submit"]')!.disabled).toBe(false);
    const select = container.querySelectorAll('select')[1];
    await act(async () => { select.value = 'interval'; select.dispatchEvent(new Event('change', { bubbles: true })); });
    await act(async () => container.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })));
    expect(JSON.parse(fetchMock.mock.calls[0][1].body)).toMatchObject({ graph: 'only', rule: { type: 'interval' } });
    expect(props.refresh).toHaveBeenCalledOnce();
  } finally {
    await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks();
  }
});
