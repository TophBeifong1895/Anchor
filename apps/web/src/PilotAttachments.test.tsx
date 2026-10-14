/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PilotAttachments } from './PilotAttachments';
import type { Attachment } from './pilotAttachments';
import { apiBlob } from './api';
vi.mock('./api', async original => ({ ...await original<object>(), apiBlob: vi.fn() }));

const attachment: Attachment = { id: 'a', name: 'plot.png', size: 4, sha256: 'a'.repeat(64), media_type: 'image/png',
  url: '/sessions/s/turns/t/attachments/a', download_url: '/sessions/s/turns/t/attachments/a?download=1' };
describe('Pilot image lifecycle', () => {
  let root: Root;
  let container: HTMLDivElement;
  const createObjectURL = vi.fn(() => 'blob:preview');
  const revokeObjectURL = vi.fn();
  beforeEach(() => {
    vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true);
    Object.defineProperty(URL, 'createObjectURL', { value: createObjectURL, configurable: true });
    Object.defineProperty(URL, 'revokeObjectURL', { value: revokeObjectURL, configurable: true });
    container = document.createElement('div'); document.body.append(container); root = createRoot(container);
    createObjectURL.mockClear(); revokeObjectURL.mockClear(); vi.mocked(apiBlob).mockReset();
  });
  afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });
  it('revokes local previews on removal and unmount instead of retaining files in object URLs', async () => {
    const draft = { id: 'draft', name: 'input.png', media_type: 'image/png', blob: new Blob(['data'], { type: 'image/png' }) };
    await act(async () => root.render(<PilotAttachments drafts={[draft]} />));
    expect(container.querySelector('img')?.getAttribute('src')).toBe('blob:preview');
    expect(apiBlob).not.toHaveBeenCalled();
    await act(async () => root.render(<PilotAttachments drafts={[]} />));
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:preview');
  });
  it('aborts a stale authenticated preview when the session changes and never creates its object URL', async () => {
    let resolve: ((blob: Blob) => void) | undefined;
    vi.mocked(apiBlob).mockImplementationOnce(() => new Promise(done => { resolve = done; }));
    await act(async () => root.render(<PilotAttachments attachments={[attachment]} session="s" turn="t" />));
    const signal = vi.mocked(apiBlob).mock.calls[0][1]!;
    await act(async () => root.render(<PilotAttachments attachments={[]} session="other" />));
    expect(signal.aborted).toBe(true);
    await act(async () => resolve!(new Blob(['data'], { type: 'image/png' })));
    expect(createObjectURL).not.toHaveBeenCalled();
  });
  it('refuses a fetched preview whose actual content type differs from Host metadata', async () => {
    vi.mocked(apiBlob).mockResolvedValue(new Blob(['<svg />'], { type: 'image/svg+xml' }));
    await act(async () => root.render(<PilotAttachments attachments={[attachment]} session="s" turn="t" />));
    expect(container.querySelector('img')).toBeNull();
    expect(container.textContent).toContain('图片格式无法预览');
    expect(createObjectURL).not.toHaveBeenCalled();
  });
});
