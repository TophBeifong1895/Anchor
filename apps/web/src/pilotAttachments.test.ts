/** @vitest-environment jsdom */
import { describe, expect, it } from 'vitest';
import { addDraftAttachments, attachmentPath, attachmentProblem, type Attachment, type DraftAttachment } from './pilotAttachments';
const MiB = 1024 * 1024;
const file = (size: number, media_type = 'text/csv', name = 'data.csv'): DraftAttachment => ({ id: 'local', name, media_type, blob: { size } as Blob });
const attachment: Attachment = { id: 'a-1', name: 'plot.png', size: 100, sha256: 'a'.repeat(64), media_type: 'image/png',
  url: '/sessions/s-1/turns/t-1/attachments/a-1', download_url: '/sessions/s-1/turns/t-1/attachments/a-1?download=1' };

describe('Pilot attachment limits', () => {
  it('keeps repeated file and pasted-image names unique while preserving extensions and bytes', () => {
    const first = new File(['first'], 'image.png', { type: 'image/png' });
    const files = addDraftAttachments([], [first, first, new File(['another'], 'image (2).png', { type: 'image/png' })]);
    const more = addDraftAttachments(files, [first]);
    expect(more.map(file => file.name)).toEqual(['image.png', 'image (2).png', 'image (2) (2).png', 'image (3).png']);
    expect(more[1].blob).toBe(first);
  });
  it.each(['image/gif', 'image/svg+xml'])('explains unsupported %s before sending', type => {
    expect(attachmentProblem([file(10, type, 'picture')])).toContain('PNG、JPEG 或 WebP');
  });
  it('allows documents and the exact image/total boundary', () => {
    expect(attachmentProblem([file(20 * MiB), file(10 * MiB, 'image/png'), file(10 * MiB, 'image/jpeg'), file(10 * MiB, 'application/pdf')])).toBe('');
  });
  it('reports per-file, file count and cumulative limits without dropping selected files', () => {
    expect(attachmentProblem([file(20 * MiB + 1)])).toContain('20 MB');
    expect(attachmentProblem([file(10 * MiB + 1, 'image/png')])).toContain('10 MB');
    expect(attachmentProblem(Array.from({ length: 17 }, () => file(1)))).toContain('16 个');
    expect(attachmentProblem(Array.from({ length: 9 }, () => file(1, 'image/webp')))).toContain('8 张');
    expect(attachmentProblem([file(20 * MiB), file(20 * MiB), file(11 * MiB)])).toContain('50 MB');
    expect(attachmentProblem([file(10 * MiB, 'image/png'), file(10 * MiB, 'image/jpeg'), file(1, 'image/webp')])).toContain('20 MB');
  });
});

describe('Host attachment URL boundary', () => {
  it('accepts only this exact attachment and download query, with same-origin absolute URLs supported', () => {
    expect(attachmentPath(attachment, 's-1', 't-1')).toBe(attachment.url);
    expect(attachmentPath(attachment, 's-1', 't-1', true)).toBe(attachment.download_url);
    expect(attachmentPath({ ...attachment, url: window.location.origin + attachment.url }, 's-1', 't-1')).toBe(attachment.url);
    expect(attachmentPath(attachment, 'other-session', 't-1')).toBeNull();
    expect(attachmentPath(attachment, 's-1', '')).toBeNull();
  });
  it.each(['https://other.test/sessions/s-1/turns/t-1/attachments/a-1', '//other.test/file',
    'data:image/png;base64,AAAA', '/graphs', '/sessions/s-1/turns/t-2/attachments/a-1',
    '/sessions/s-1/turns/t-1/attachments/a-2', '/sessions/s-1/turns/t-1/attachments/a-1?key=secret',
    '/sessions/s-1/turns/t-1/attachments/a-1#fragment', '/sessions/s-1/turns/t-1/attachments/a-1?download=1'])('refuses %s for previews', url => {
    expect(attachmentPath({ ...attachment, url }, 's-1', 't-1')).toBeNull();
  });
  it('refuses extra, reordered or missing download parameters', () => {
    for (const download_url of [attachment.url, attachment.url + '?download=1&key=secret', attachment.url + '?download=true']) {
      expect(attachmentPath({ ...attachment, download_url }, 's-1', 't-1', true)).toBeNull();
    }
  });
});
