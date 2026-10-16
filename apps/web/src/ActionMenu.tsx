import { useEffect, useId, useRef, useState, type ReactNode, type RefObject } from 'react';
import { MoreHorizontal } from 'lucide-react';
import './ActionMenu.css';

/** Native light-dismiss/Escape behavior and one keyboard/focus contract for row actions. */
export function ActionMenu({ label, menuLabel, triggerRef, children }: {
  label: string; menuLabel: string; triggerRef: RefObject<HTMLButtonElement | null>;
  children: (close: () => void) => ReactNode;
}) {
  const id = useId();
  const menu = useRef<HTMLDivElement>(null);
  const initialItem = useRef<'first' | 'last'>('first');
  const [open, setOpen] = useState(false);
  const close = () => menu.current?.hidePopover();
  useEffect(() => {
    if (!open) return;
    const dismiss = (event: Event) => {
      if (event.type !== 'scroll' || !(event.target instanceof Node) || !menu.current?.contains(event.target)) close();
    };
    window.addEventListener('resize', dismiss);
    document.addEventListener('scroll', dismiss, true);
    return () => {
      window.removeEventListener('resize', dismiss);
      document.removeEventListener('scroll', dismiss, true);
    };
  }, [open]);
  const buttons = () => [...(menu.current?.querySelectorAll<HTMLButtonElement>('button[role="menuitem"]:not(:disabled)') ?? [])];
  return <>
    <button ref={triggerRef} type="button" className="action-menu-trigger" aria-label={label}
      aria-haspopup="menu" aria-expanded={open} aria-controls={id} popoverTarget={id}
      onKeyDown={event => {
        if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
        event.preventDefault();
        initialItem.current = event.key === 'ArrowUp' ? 'last' : 'first';
        menu.current?.showPopover();
      }}>
      <MoreHorizontal size={16} />
    </button>
    <div ref={menu} id={id} popover="auto" role="menu" aria-label={menuLabel} tabIndex={-1}
      className="action-menu" onToggle={event => {
        const visible = event.newState === 'open';
        setOpen(visible);
        if (!visible || !menu.current || !triggerRef.current) return;
        const anchor = triggerRef.current.getBoundingClientRect();
        const bounds = menu.current.getBoundingClientRect();
        const below = anchor.bottom + 6;
        const top = below + bounds.height <= window.innerHeight ? below : anchor.top - bounds.height - 6;
        menu.current.style.left = `${Math.max(8, Math.min(anchor.right - bounds.width, window.innerWidth - bounds.width - 8))}px`;
        menu.current.style.top = `${Math.max(8, Math.min(top, window.innerHeight - bounds.height - 8))}px`;
        const items = buttons();
        (initialItem.current === 'last' ? items.at(-1) ?? menu.current : items[0] ?? menu.current).focus();
        initialItem.current = 'first';
      }} onKeyDown={event => {
        if (event.key === 'Escape') {
          event.preventDefault(); close(); triggerRef.current?.focus(); return;
        }
        const items = buttons();
        if (!items.length) return;
        const index = items.indexOf(document.activeElement as HTMLButtonElement);
        const next = event.key === 'ArrowDown' ? (index + 1) % items.length : event.key === 'ArrowUp'
          ? (index < 0 ? items.length - 1 : (index - 1 + items.length) % items.length)
          : event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : -1;
        if (next >= 0) { event.preventDefault(); items[next].focus(); }
      }}>{children(close)}</div>
  </>;
}
