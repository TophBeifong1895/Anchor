/** @vitest-environment jsdom */
import { act, useRef } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ActionMenu } from './ActionMenu';
import { GraphActions } from './GraphActions';
import { SessionActions } from './SessionActions';
import { api } from './api';

vi.mock('./api', () => ({ api: vi.fn(), reason: (cause: unknown) => String(cause instanceof Error ? cause.message : cause) }));

function toggle(element: HTMLElement, state: 'open' | 'closed') {
  element.dispatchEvent(Object.assign(new Event('toggle'), { newState: state }));
}

describe('shared row actions and version-bound Graph deletion', () => {
  let container: HTMLDivElement;
  let root: Root;
  beforeEach(() => {
    vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true);
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { callback(0); return 1; });
    vi.mocked(api).mockReset();
    Object.defineProperties(HTMLElement.prototype, {
      showPopover: { configurable: true, value: function (this: HTMLElement) { this.removeAttribute('popover'); toggle(this, 'open'); } },
      hidePopover: { configurable: true, value: function (this: HTMLElement) { this.setAttribute('popover', 'auto'); toggle(this, 'closed'); } },
    });
    Object.defineProperties(HTMLDialogElement.prototype, {
      showModal: { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute('open', ''); } },
      close: { configurable: true, value: function (this: HTMLDialogElement) { this.removeAttribute('open'); } },
    });
    container = document.createElement('div'); document.body.appendChild(container);
    root = createRoot(container);
  });
  afterEach(async () => {
    await act(async () => root.unmount()); container.remove();
    vi.unstubAllGlobals();
  });
  const key = async (element: HTMLElement, value: string) => {
    await act(async () => element.dispatchEvent(new KeyboardEvent('keydown', { key: value, bubbles: true })));
  };

  it('shares arrow/Home/End/Escape focus handling, skips disabled actions and dismisses on resize', async () => {
    function Harness() {
      const trigger = useRef<HTMLButtonElement>(null);
      return <ActionMenu label="manage" menuLabel="actions" triggerRef={trigger}>{() => <>
        <button role="menuitem">first</button><button role="menuitem" disabled>disabled</button><button role="menuitem">last</button>
      </>}</ActionMenu>;
    }
    await act(async () => root.render(<Harness />));
    const trigger = container.querySelector<HTMLButtonElement>('[aria-haspopup="menu"]')!;
    const menu = container.querySelector<HTMLElement>('[role="menu"]')!;
    const items = container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]');
    await key(trigger, 'ArrowUp'); expect(items[2]).toBe(document.activeElement);
    expect(trigger.getAttribute('aria-expanded')).toBe('true');
    await key(menu, 'ArrowDown'); expect(items[0]).toBe(document.activeElement);
    await key(menu, 'End'); expect(items[2]).toBe(document.activeElement);
    await key(menu, 'Home'); expect(items[0]).toBe(document.activeElement);
    await key(menu, 'Escape'); expect(trigger).toBe(document.activeElement);
    expect(trigger.getAttribute('aria-expanded')).toBe('false');
    await key(trigger, 'ArrowDown');
    await act(async () => window.dispatchEvent(new Event('resize')));
    expect(trigger.getAttribute('aria-expanded')).toBe('false');
  });

  it('loads the delete version before enabling confirmation and keeps that version when deleting', async () => {
    let finish!: (value: { graph: string; precondition: string }) => void;
    vi.mocked(api).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    const remove = vi.fn().mockRejectedValue(new Error('Graph changed; request a new confirmation'));
    await act(async () => root.render(<GraphActions graph="demo" runCount={0} busy={false} running={false} onDelete={remove} />));
    await act(async () => container.querySelector<HTMLButtonElement>('[role="menuitem"]')!.click());
    const confirm = container.querySelector<HTMLButtonElement>('.confirm-delete')!;
    expect(confirm.disabled).toBe(true);
    expect(vi.mocked(api).mock.calls[0][0]).toBe('/graphs/demo/delete-precondition');
    await act(async () => finish({ graph: 'demo', precondition: 'version-before-confirmation' }));
    expect(confirm.disabled).toBe(false);
    await act(async () => confirm.click());
    expect(remove).toHaveBeenCalledWith('demo', 'version-before-confirmation');
    expect(api).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain('Graph changed');
  });

  it('refuses deletion when the precondition cannot be read', async () => {
    vi.mocked(api).mockRejectedValue(new Error('Graph not available'));
    const remove = vi.fn();
    await act(async () => root.render(<GraphActions graph="demo" runCount={0} busy={false} running={false} onDelete={remove} />));
    await act(async () => container.querySelector<HTMLButtonElement>('[role="menuitem"]')!.click());
    expect(container.querySelector<HTMLButtonElement>('.confirm-delete')!.disabled).toBe(true);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain('Graph not available');
    expect(remove).not.toHaveBeenCalled();
  });

  it('keeps Session rename/delete behavior in the shared menu with disabled deletion while busy', async () => {
    const rename = vi.fn().mockResolvedValue(undefined);
    const remove = vi.fn();
    await act(async () => root.render(<SessionActions title="Pilot" busy onRename={rename} onDelete={remove} />));
    const items = container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]');
    expect(items[1].disabled).toBe(true);
    await act(async () => items[0].click());
    const field = container.querySelector<HTMLInputElement>('input')!;
    expect(field).toBe(document.activeElement);
    await act(async () => container.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })));
    expect(rename).toHaveBeenCalledWith('Pilot');
    expect(remove).not.toHaveBeenCalled();
  });
});
