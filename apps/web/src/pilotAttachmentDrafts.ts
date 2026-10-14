import { attachmentProblem, type DraftAttachment } from './pilotAttachments';

const databaseName = 'anchor-pilot-drafts';
const storeName = 'attachments';
const maxStoredBytes = 100 * 1024 * 1024;
const maxStoredFiles = 160;
type DraftRecord = { scope: string; files: DraftAttachment[] };
let database: Promise<IDBDatabase> | undefined;
function open(): Promise<IDBDatabase> {
  if (!database) database = new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(databaseName, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(storeName, { keyPath: 'scope' });
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(new Error('无法保存附件草稿；请检查浏览器存储权限。'));
  }).catch(error => { database = undefined; throw error; });
  return database;
}
export async function readAttachmentDraft(scope: string): Promise<DraftAttachment[]> {
  const db = await open();
  return new Promise((resolve, reject) => {
    const request = db.transaction(storeName).objectStore(storeName).get(scope);
    request.onsuccess = () => {
      const files = (request.result as DraftRecord | undefined)?.files ?? [];
      if (!Array.isArray(files) || files.some(file => !(file.blob instanceof Blob) || typeof file.name !== 'string' || typeof file.id !== 'string') || attachmentProblem(files)) {
        reject(new Error('附件草稿无法读取，请重新选择文件。')); return;
      }
      resolve(files);
    };
    request.onerror = () => reject(new Error('无法读取附件草稿，请重新选择文件。'));
  });
}
export async function saveAttachmentDraft(scope: string, files: DraftAttachment[], moveFrom?: string): Promise<void> {
  const invalid = attachmentProblem(files);
  if (invalid) throw new Error(invalid);
  const db = await open();
  return new Promise((resolve, reject) => {
    const transaction = db.transaction(storeName, 'readwrite');
    const store = transaction.objectStore(storeName);
    let problem = '附件草稿未保存，刷新前请重试或重新选择文件。';
    const request = store.getAll();
    request.onsuccess = () => {
      const records = request.result as DraftRecord[];
      const source = moveFrom ? records.find(record => record.scope === moveFrom) : undefined;
      const moving = source && source.files.length === files.length && source.files.every((file, index) => file.id === files[index].id);
      const others = records.filter(record => record.scope !== scope && !(moving && record.scope === moveFrom));
      const otherBytes = others.reduce((total, record) => total + record.files.reduce((size, file) => size + file.blob.size, 0), 0);
      if (otherBytes + files.reduce((size, file) => size + file.blob.size, 0) > maxStoredBytes) {
        problem = '浏览器中的附件草稿超过 100 MB，请移除其他对话中未发送的附件。';
        transaction.abort(); return;
      }
      if (others.reduce((total, record) => total + record.files.length, 0) + files.length > maxStoredFiles) {
        problem = '浏览器中的附件草稿超过 160 个文件，请移除其他对话中未发送的附件。';
        transaction.abort(); return;
      }
      if (files.length) store.put({ scope, files } satisfies DraftRecord);
      else store.delete(scope);
      if (moving) store.delete(moveFrom!);
    };
    transaction.oncomplete = () => resolve();
    transaction.onabort = transaction.onerror = () => reject(new Error(problem));
  });
}
/** Clear only the sent snapshot: a late response must not remove a newer draft. */
export async function clearAttachmentDraft(scope: string, ids?: string[]): Promise<void> {
  const db = await open();
  return new Promise((resolve, reject) => {
    const transaction = db.transaction(storeName, 'readwrite');
    const store = transaction.objectStore(storeName);
    const request = store.get(scope);
    request.onsuccess = () => {
      const saved = (request.result as DraftRecord | undefined)?.files ?? [];
      if (!ids || (saved.length === ids.length && saved.every((file, index) => file.id === ids[index]))) store.delete(scope);
    };
    transaction.oncomplete = () => resolve();
    transaction.onabort = transaction.onerror = () => reject(new Error('无法清除已发送的附件草稿。'));
  });
}
