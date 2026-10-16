import { useEffect, useState } from 'react';
import { FileText, LoaderCircle, RefreshCw, X } from 'lucide-react';
import { apiBlob, human, reason } from './api';
import { attachmentPath, imageTypes, type Attachment, type DraftAttachment } from './pilotAttachments';
import { DownloadButton } from './DownloadButton';
import './PilotAttachments.css';

type Source = { attachment: Attachment; session: string; turn: string } | { draft: DraftAttachment };
function AttachmentCard({ source, onRemove, disabled = false }: { source: Source; onRemove?: () => void; disabled?: boolean }) {
  const local = 'draft' in source ? source.draft : undefined;
  const remote = 'attachment' in source ? source : undefined;
  const name = local?.name ?? remote!.attachment.name;
  const size = local?.blob.size ?? remote!.attachment.size;
  const mediaType = local?.media_type ?? remote?.attachment.media_type;
  const image = imageTypes.has(mediaType ?? '');
  const previewPath = remote ? attachmentPath(remote.attachment, remote.session, remote.turn) : undefined;
  const downloadPath = remote ? attachmentPath(remote.attachment, remote.session, remote.turn, true) : undefined;
  const [preview, setPreview] = useState('');
  const [previewProblem, setPreviewProblem] = useState('');
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    if (!image) return;
    const controller = new AbortController();
    let url = '';
    setPreview(''); setPreviewProblem('');
    void (async () => {
      try {
        if (remote && !previewPath) throw new Error('图片链接不可用。');
        const blob = local?.blob ?? await apiBlob(previewPath!, controller.signal);
        if (!imageTypes.has(blob.type) || (mediaType && blob.type !== mediaType)) throw new Error('图片格式无法预览，请下载文件查看。');
        if (controller.signal.aborted) return;
        url = URL.createObjectURL(blob); setPreview(url);
      } catch (error) { if (!controller.signal.aborted) setPreviewProblem(reason(error)); }
    })();
    return () => { controller.abort(); if (url) URL.revokeObjectURL(url); };
  }, [local?.blob, previewPath, mediaType, image, revision]);
  return <div className={`pilot-attachment${image ? ' is-image' : ''}`}>
    {image && <div className="pilot-attachment-preview">
      {preview && !previewProblem && <img src={preview} alt={name} onError={() => setPreviewProblem('图片加载失败，请重试或下载查看。')} />}
      {previewProblem ? <div className="pilot-attachment-unavailable"><span>{previewProblem}</span><button type="button" onClick={() => setRevision(value => value + 1)}><RefreshCw size={13} />重试图片</button></div>
        : !preview && <span className="pilot-attachment-loading" role="status"><LoaderCircle size={16} />加载图片…</span>}
    </div>}
    <div className="pilot-attachment-details"><FileText size={15} aria-hidden="true" /><span><strong title={name}>{name}</strong><small>{human(size)}</small></span>
      <DownloadButton className="pilot-attachment-action" errorClassName="pilot-attachment-error" name={name}
        path={downloadPath ?? undefined} blob={local?.blob} disabled={!!remote && !downloadPath} />
      {onRemove && <button type="button" className="pilot-attachment-action" aria-label={`移除 ${name}`} title="移除" disabled={disabled} onClick={onRemove}><X size={15} /></button>}
    </div>
    {remote && !downloadPath && <p className="pilot-attachment-error">文件链接不可用，请重新加载对话。</p>}
  </div>;
}
export function PilotAttachments({ attachments = [], drafts = [], session = '', turn = '', onRemove, disabled }: {
  attachments?: Attachment[]; drafts?: DraftAttachment[]; session?: string; turn?: string;
  onRemove?: (id: string) => void; disabled?: boolean;
}) {
  if (!attachments.length && !drafts.length) return null;
  return <div className="pilot-attachments" aria-label={onRemove ? '已选附件' : '附件'}>
    {attachments.map(attachment => <AttachmentCard key={attachment.id} source={{ attachment, session, turn }} />)}
    {drafts.map(draft => <AttachmentCard key={draft.id} source={{ draft }} disabled={disabled} onRemove={onRemove ? () => onRemove(draft.id) : undefined} />)}
  </div>;
}
