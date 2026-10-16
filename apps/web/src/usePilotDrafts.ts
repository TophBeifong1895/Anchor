import { useEffect, useRef, useState, type RefObject } from 'react';
import type { DraftAttachment } from './pilotAttachments';
import { clearAttachmentDraft, readAttachmentDraft, saveAttachmentDraft } from './pilotAttachmentDrafts';

const cached = (key: string, value?: string) => {
  try {
    if (value !== undefined) localStorage.setItem(`pilot:${key}`, value);
    return localStorage.getItem(`pilot:${key}`) ?? '';
  } catch { return ''; }
};
const scope = (id: string) => id ? `session:${id}` : 'new';

/** Owns only unsent text/files; revisions make navigation and late IDB writes harmless. */
export function usePilotDrafts(selected: string, selectedRef: RefObject<string>, mounted: RefObject<boolean>) {
  const [draft, setDraft] = useState(() => cached('draft:new'));
  const [files, setFiles] = useState<DraftAttachment[]>([]);
  const filesRef = useRef<DraftAttachment[]>([]);
  const [filesReady, setFilesReady] = useState(false);
  const [attachmentError, setAttachmentError] = useState('');
  const [loadVersion, setLoadVersion] = useState(0);
  const revision = useRef(0);
  const setCurrentFiles = (value: DraftAttachment[]) => { filesRef.current = value; setFiles(value); };
  const storeFiles = async (id: string, value: DraftAttachment[]) => {
    const current = ++revision.current;
    setCurrentFiles(value); setAttachmentError('');
    try { await saveAttachmentDraft(scope(id), value); }
    catch (error) {
      if (mounted.current && selectedRef.current === id && current === revision.current) setAttachmentError((error as Error).message);
    }
  };
  useEffect(() => {
    const id = selected;
    const current = ++revision.current;
    setFilesReady(false); setCurrentFiles([]); setAttachmentError('');
    void readAttachmentDraft(scope(id)).then(value => {
      if (mounted.current && selectedRef.current === id && current === revision.current) setCurrentFiles(value);
    }).catch(error => {
      if (mounted.current && selectedRef.current === id && current === revision.current) setAttachmentError((error as Error).message);
    }).finally(() => {
      if (mounted.current && selectedRef.current === id && current === revision.current) setFilesReady(true);
    });
  }, [selected, loadVersion]);
  const activate = (id: string) => {
    ++revision.current; setCurrentFiles([]); setFilesReady(false); setAttachmentError('');
    setLoadVersion(value => value + 1); setDraft(cached(`draft:${id || 'new'}`));
  };
  const editDraft = (value: string) => { cached(`draft:${selectedRef.current || 'new'}`, value); setDraft(value); };
  const clearSent = async (id: string, ids: string[]) => {
    await clearAttachmentDraft(scope(id), ids);
    if (mounted.current && selectedRef.current === id && ids.length === filesRef.current.length
      && ids.every((value, index) => filesRef.current[index].id === value)) setCurrentFiles([]);
  };
  return { draft, setDraft, files, filesRef, filesReady, setFilesReady, attachmentError, setAttachmentError,
    setCurrentFiles, storeFiles, editDraft, activate, clearSent };
}
