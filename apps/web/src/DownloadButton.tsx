import { useEffect, useRef, useState } from 'react';
import { Download, LoaderCircle } from 'lucide-react';
import { apiBlob, reason } from './api';
import './DownloadButton.css';

/** One authenticated download flow for Run files, retained attachments and drafts. */
export function DownloadButton({ name, path, blob, disabled, className, errorClassName = 'message error', size = 15 }: {
  name: string; path?: string; blob?: Blob; disabled?: boolean; className?: string; errorClassName?: string; size?: number;
}) {
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState('');
  const request = useRef<AbortController | null>(null);
  const urls = useRef(new Set<string>());
  useEffect(() => {
    setProblem(''); setBusy(false);
    return () => {
      request.current?.abort(); request.current = null;
      for (const url of urls.current) URL.revokeObjectURL(url);
      urls.current.clear();
    };
  }, [path, blob]);
  const download = async () => {
    if (request.current) return;
    const controller = new AbortController(); request.current = controller;
    setBusy(true); setProblem('');
    try {
      if (!blob && !path) throw new Error('文件链接不可用，请重新加载后重试。');
      const content = blob ?? await apiBlob(path!, controller.signal);
      if (controller.signal.aborted) return;
      const url = URL.createObjectURL(content); urls.current.add(url);
      const link = document.createElement('a');
      link.href = url; link.download = name; document.body.append(link); link.click(); link.remove();
      window.setTimeout(() => { URL.revokeObjectURL(url); urls.current.delete(url); }, 1000);
    } catch (error) { if (!controller.signal.aborted) setProblem(reason(error)); }
    finally {
      if (request.current === controller) request.current = null;
      if (!controller.signal.aborted) setBusy(false);
    }
  };
  return <>
    <button type="button" className={className} aria-label={`下载 ${name}`} title={`下载 ${name}`}
      disabled={busy || disabled} onClick={() => void download()}>
      {busy ? <LoaderCircle size={size} className="spin" /> : <Download size={size} />}
    </button>
    {problem && <p className={`download-error ${errorClassName}`} role="alert">{problem}</p>}
  </>;
}
