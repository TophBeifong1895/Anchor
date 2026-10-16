import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';

const WIDTH_KEY = 'anchor.pilot.content-width';
const DEFAULT_WIDTH = 800;
const MIN_WIDTH = 480;
const EDGE_SPACE = 48;

function savedWidth(): number | null {
  try {
    const value = Number(localStorage.getItem(WIDTH_KEY));
    return Number.isFinite(value) && value > 0 ? value : null;
  } catch { return null; }
}

/** A reading preference only; resizing never changes Session, Turn or Plugin facts. */
export function PilotConversationLayout({ start, children }: { start: boolean; children: ReactNode }) {
  const body = useRef<HTMLDivElement>(null);
  const [available, setAvailable] = useState(0);
  const [preference, setPreference] = useState(savedWidth);
  const [preview, setPreview] = useState<number | null>(null);
  const drag = useRef<{ pointer: number; x: number; width: number } | null>(null);
  const maximum = Math.max(0, available - EDGE_SPACE);
  const minimum = Math.min(MIN_WIDTH, maximum);
  const clamp = (value: number) => Math.round(Math.max(minimum, Math.min(maximum, value)));
  const adjustable = !start && available > 640;
  const width = available ? (available <= 640 ? available : clamp(preview ?? preference ?? DEFAULT_WIDTH)) : DEFAULT_WIDTH;
  useLayoutEffect(() => {
    const element = body.current;
    if (!element) return;
    const measure = () => setAvailable(element.clientWidth);
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    measure();
    return () => observer.disconnect();
  }, []);
  useLayoutEffect(() => {
    if (!adjustable) { drag.current = null; setPreview(null); }
  }, [adjustable]);
  const save = (value: number | null) => {
    setPreference(value);
    try {
      if (value === null) localStorage.removeItem(WIDTH_KEY);
      else localStorage.setItem(WIDTH_KEY, String(value));
    } catch { /* The current layout still works when browser storage is unavailable. */ }
  };
  return <div ref={body} className={`pilot-chat-body${start ? ' is-start' : ''}`}
    style={{ '--conversation-width': `${width}px` } as CSSProperties}>
    {children}
    {adjustable && (['left', 'right'] as const).map(side => {
      const direction = side === 'left' ? -1 : 1;
      return <div key={side} className={`pilot-width-handle is-${side}`} role="separator" tabIndex={0}
        aria-label={`调整对话宽度（${side === 'left' ? '左' : '右'}侧）`} aria-orientation="vertical"
        aria-valuemin={minimum} aria-valuemax={maximum} aria-valuenow={width}
        title="拖动调整对话宽度；双击或 Home 恢复默认，方向键微调"
        onDoubleClick={() => save(null)}
        onPointerDown={event => {
          if (event.button !== 0 || drag.current) return;
          event.preventDefault();
          event.currentTarget.focus({ preventScroll: true });
          event.currentTarget.setPointerCapture(event.pointerId);
          drag.current = { pointer: event.pointerId, x: event.clientX, width };
        }}
        onPointerMove={event => {
          const current = drag.current;
          if (!current || current.pointer !== event.pointerId || !event.currentTarget.hasPointerCapture(event.pointerId)) return;
          // Both edges move equally so the text and composer keep the same center.
          setPreview(clamp(current.width + (event.clientX - current.x) * direction * 2));
        }}
        onPointerUp={event => {
          const current = drag.current;
          if (!current || current.pointer !== event.pointerId) return;
          drag.current = null;
          // Clicking a clamped edge must not replace the user's wider saved preference.
          if (event.clientX !== current.x) save(clamp(current.width + (event.clientX - current.x) * direction * 2));
          setPreview(null);
          if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
        }}
        onPointerCancel={() => { drag.current = null; setPreview(null); }}
        onLostPointerCapture={() => { drag.current = null; setPreview(null); }}
        onWheel={event => {
          if (event.ctrlKey || !event.deltaY) return;
          const scrollport = body.current?.querySelector<HTMLElement>('.pilot-messages');
          if (!scrollport) return;
          const lineHeight = Number.parseFloat(getComputedStyle(scrollport).lineHeight) || 16;
          const unit = event.deltaMode === 1 ? lineHeight : event.deltaMode === 2 ? scrollport.clientHeight : 1;
          scrollport.scrollBy({ top: event.deltaY * unit });
        }}
        onKeyDown={event => {
          if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
          event.preventDefault();
          if (event.key === 'Home') save(null);
          else save(event.key === 'End' ? maximum : clamp(width + (event.key === 'ArrowRight' ? 16 : -16) * direction));
        }} />;
    })}
  </div>;
}
