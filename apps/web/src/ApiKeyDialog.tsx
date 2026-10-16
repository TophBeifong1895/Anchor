import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { reason, registerApiKeyPrompt, setBearerKey, verifyBearerKey } from './api';
import { Modal } from './ui';

/** Host authentication is one application-wide interaction, independent of page subscriptions. */
export function ApiKeyDialog() {
  const [apiKeyDialog, setApiKeyDialog] = useState(false);
  const [apiKeyDraft, setApiKeyDraft] = useState('');
  const [apiKeyProblem, setApiKeyProblem] = useState('');
  const [apiKeyChecking, setApiKeyChecking] = useState(false);
  const apiKeyResolver = useRef<((key: string) => void) | null>(null);
  const apiKeyCheck = useRef<AbortController | null>(null);
  // Register in layout phase so the App's first data effect can open the dialog.
  useLayoutEffect(() => registerApiKeyPrompt(() => new Promise(resolve => {
    apiKeyResolver.current = resolve;
    setApiKeyDraft('');
    setApiKeyProblem('');
    setApiKeyDialog(true);
  })), []);

  const finishApiKeyPrompt = (key: string) => {
    apiKeyCheck.current?.abort();
    apiKeyCheck.current = null;
    setApiKeyChecking(false);
    if (key) setBearerKey(key);
    apiKeyResolver.current?.(key);
    apiKeyResolver.current = null;
    setApiKeyDialog(false);
  };

  const submitApiKey = async () => {
    if (apiKeyChecking) return;
    const controller = new AbortController();
    apiKeyCheck.current = controller;
    setApiKeyChecking(true);
    setApiKeyProblem('');
    try {
      const key = await verifyBearerKey(apiKeyDraft, controller.signal);
      if (!controller.signal.aborted) finishApiKeyPrompt(key);
    } catch (error) {
      if (!controller.signal.aborted) setApiKeyProblem(reason(error));
    } finally {
      if (apiKeyCheck.current === controller) {
        apiKeyCheck.current = null;
        setApiKeyChecking(false);
      }
    }
  };

  useEffect(() => () => {
    apiKeyCheck.current?.abort(); apiKeyResolver.current?.('');
  }, []);
  return apiKeyDialog ? <Modal title="连接 Anchor 服务" close={() => finishApiKeyPrompt('')}>
        <form className="api-key-form" onSubmit={event => {
          event.preventDefault();
          void submitApiKey();
        }}>
          <p>此 Anchor 服务需要 API key。密钥只保存在当前浏览器标签页中。</p>
          <label htmlFor="anchor-api-key">Anchor API key</label>
          <input id="anchor-api-key" type="password" autoComplete="current-password" autoFocus
            aria-invalid={!!apiKeyProblem} aria-describedby={apiKeyProblem ? 'anchor-api-key-error' : undefined}
            value={apiKeyDraft} onChange={event => { setApiKeyDraft(event.target.value); setApiKeyProblem(''); }} />
          {apiKeyProblem && <p id="anchor-api-key-error" className="notice bad" role="alert">{apiKeyProblem}</p>}
          <div className="modal-actions">
            <button type="button" onClick={() => finishApiKeyPrompt('')}>取消</button>
            <button className="primary" type="submit" disabled={apiKeyChecking || !apiKeyDraft.trim()}>{apiKeyChecking ? '验证中…' : '连接'}</button>
          </div>
        </form>
      </Modal> : null;
}
