export type Attachment = {
  id: string; name: string; size: number; sha256: string;
  media_type?: string | null; url: string; download_url: string;
};
export type DraftAttachment = { id: string; name: string; media_type?: string; blob: Blob };
export type AttachmentInput = { name: string; data_base64: string; media_type?: string };
const MiB = 1024 * 1024;
export const imageTypes = new Set(['image/png', 'image/jpeg', 'image/webp']);

export function attachmentProblem(files: DraftAttachment[]): string {
  if (files.length > 16) return '一次最多添加 16 个文件，请移除多余文件。';
  const images = files.filter(file => imageTypes.has(file.media_type ?? ''));
  if (images.length > 8) return '一次最多添加 8 张图片，请移除多余图片。';
  for (const file of files) {
    if (file.media_type?.startsWith('image/') && !imageTypes.has(file.media_type)) return `「${file.name}」的图片格式暂不支持，请使用 PNG、JPEG 或 WebP。`;
    if (file.blob.size > 20 * MiB) return `「${file.name}」超过 20 MB，请选择较小的文件。`;
    if (imageTypes.has(file.media_type ?? '') && file.blob.size > 10 * MiB) return `「${file.name}」超过图片的 10 MB 限制。`;
  }
  if (files.reduce((size, file) => size + file.blob.size, 0) > 50 * MiB) return '附件总大小超过 50 MB，请移除部分文件。';
  if (images.reduce((size, file) => size + file.blob.size, 0) > 20 * MiB) return '图片总大小超过 20 MB，请移除部分图片。';
  return '';
}
export function draftAttachment(file: File): DraftAttachment {
  return { id: crypto.randomUUID(), name: file.name || '粘贴的图片.png', media_type: file.type || undefined, blob: file };
}
export function addDraftAttachments(existing: DraftAttachment[], incoming: File[]): DraftAttachment[] {
  const names = new Set(existing.map(file => file.name));
  return [...existing, ...incoming.map(file => {
    const draft = draftAttachment(file);
    const dot = draft.name.lastIndexOf('.');
    const base = dot > 0 ? draft.name.slice(0, dot) : draft.name;
    const extension = dot > 0 ? draft.name.slice(dot) : '';
    let count = 2;
    while (names.has(draft.name)) draft.name = `${base} (${count++})${extension}`;
    names.add(draft.name);
    return draft;
  })];
}
export function encodeAttachment(file: DraftAttachment): Promise<AttachmentInput> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error(`无法读取「${file.name}」，请重新选择文件。`));
    reader.onload = () => {
      const value = String(reader.result ?? '');
      const comma = value.indexOf(',');
      if (comma < 0) { reject(new Error(`无法读取「${file.name}」。`)); return; }
      resolve({ name: file.name, data_base64: value.slice(comma + 1), ...(file.media_type ? { media_type: file.media_type } : {}) });
    };
    reader.readAsDataURL(file.blob);
  });
}

/** Only Host-issued URLs for this exact attachment may carry the tab's API key. */
export function attachmentPath(attachment: Attachment, session: string, turn: string, download = false): string | null {
  if (!attachment.id || !session || !turn) return null;
  const expected = `/sessions/${encodeURIComponent(session)}/turns/${encodeURIComponent(turn)}/attachments/${encodeURIComponent(attachment.id)}`;
  try {
    const url = new URL(download ? attachment.download_url : attachment.url, window.location.origin);
    if (url.origin !== window.location.origin || url.username || url.password || url.hash
      || url.pathname !== expected || url.search !== (download ? '?download=1' : '')) return null;
    return expected + (download ? '?download=1' : '');
  } catch { return null; }
}
