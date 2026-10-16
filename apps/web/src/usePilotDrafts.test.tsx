/** @vitest-environment jsdom */
import { act, useRef } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { usePilotDrafts } from './usePilotDrafts';
import { readAttachmentDraft, saveAttachmentDraft } from './pilotAttachmentDrafts';
import type { DraftAttachment } from './pilotAttachments';

vi.mock('./pilotAttachmentDrafts', () => ({ readAttachmentDraft: vi.fn(), saveAttachmentDraft: vi.fn(), clearAttachmentDraft: vi.fn() }));
describe('Pilot draft ownership', () => {
  let root: Root;
  let state: ReturnType<typeof usePilotDrafts>;
  const Harness = ({ selected }: { selected: string }) => {
    const current = useRef(selected); current.current = selected;
    const mounted = useRef(true);
    state = usePilotDrafts(selected, current, mounted); return null;
  };
  const file = (id: string): DraftAttachment => ({ id, name: `${id}.txt`, blob: new Blob([id]), media_type: 'text/plain' });
  beforeEach(() => {
    vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true);
    root = createRoot(document.createElement('div')); localStorage.clear();
    vi.mocked(readAttachmentDraft).mockReset(); vi.mocked(saveAttachmentDraft).mockReset();
  });
  afterEach(async () => { await act(async () => root.unmount()); vi.unstubAllGlobals(); });
  it('keeps the selected session attachment draft when the previous load finishes later', async () => {
    let release!: (files: DraftAttachment[]) => void;
    vi.mocked(readAttachmentDraft).mockImplementation(scope => scope === 'session:first'
      ? new Promise(resolve => { release = resolve; }) : Promise.resolve([file('second')]));
    await act(async () => root.render(<Harness selected="first" />));
    await act(async () => root.render(<Harness selected="second" />));
    await act(async () => release([file('first')]));
    expect(state.files.map(item => item.id)).toEqual(['second']); expect(state.filesReady).toBe(true);
  });
  it('suppresses a stale persistence failure after a newer attachment edit', async () => {
    vi.mocked(readAttachmentDraft).mockResolvedValue([]);
    let fail!: (error: Error) => void;
    vi.mocked(saveAttachmentDraft).mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; })).mockResolvedValue(undefined);
    await act(async () => root.render(<Harness selected="first" />));
    let old!: Promise<void>;
    await act(async () => { old = state.storeFiles('first', [file('old')]); });
    await act(async () => state.storeFiles('first', [file('new')]));
    await act(async () => { fail(new Error('old write failed')); await old; });
    expect(state.files.map(item => item.id)).toEqual(['new']); expect(state.attachmentError).toBe('');
  });
});
