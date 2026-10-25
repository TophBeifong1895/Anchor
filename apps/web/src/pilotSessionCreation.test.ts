/** @vitest-environment jsdom */
import { webcrypto } from 'node:crypto';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { api } from './api';

vi.mock('./api', () => ({ api: vi.fn() }));

beforeEach(() => {
  vi.resetModules();
  vi.mocked(api).mockReset();
  localStorage.clear();
  vi.stubGlobal('crypto', webcrypto);
});

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it('retains a creation key across failure and reload, clears it only on success', async () => {
  let { createPilotSession } = await import('./pilotSessionCreation');
  vi.mocked(api).mockRejectedValueOnce(new Error('response lost'));
  await expect(createPilotSession()).rejects.toThrow('response lost');
  const headers = vi.mocked(api).mock.calls[0][4];
  expect(headers?.['Idempotency-Key']).toBeTruthy();
  expect(localStorage.length).toBe(1);
  expect(localStorage.key(0)).not.toContain('owner-a');
  vi.resetModules();
  ({ createPilotSession } = await import('./pilotSessionCreation'));
  vi.mocked(api).mockResolvedValue({ session: { id: 'retained' } });
  await expect(createPilotSession()).resolves.toEqual({ session: { id: 'retained' } });
  expect(vi.mocked(api).mock.calls[1][4]).toEqual(headers);
  expect(localStorage.length).toBe(0);
  await createPilotSession();
  expect(vi.mocked(api).mock.calls[2][4]).not.toEqual(headers);
});

it('keeps the key when a successful response lacks the created session identity', async () => {
  const { createPilotSession } = await import('./pilotSessionCreation');
  vi.mocked(api).mockResolvedValueOnce({});
  await expect(createPilotSession()).rejects.toThrow('未返回有效会话');
  const headers = vi.mocked(api).mock.calls[0][4];
  vi.mocked(api).mockResolvedValueOnce({ session: { id: 'retained' } });
  await createPilotSession();
  expect(vi.mocked(api).mock.calls[1][4]).toEqual(headers);
});

it.each([null, '', '   ', 42, {}])('retains the creation key for an invalid session identity: %j', async id => {
  const { createPilotSession } = await import('./pilotSessionCreation');
  vi.mocked(api).mockResolvedValueOnce({ session: { id } });
  await expect(createPilotSession()).rejects.toThrow('未返回有效会话');
  const headers = vi.mocked(api).mock.calls[0][4];
  vi.mocked(api).mockResolvedValueOnce({ session: { id: 'retained' } });
  await createPilotSession();
  expect(vi.mocked(api).mock.calls[1][4]).toEqual(headers);
});

it('retries with the in-memory key when browser storage is unavailable', async () => {
  for (const method of ['getItem', 'setItem', 'removeItem'] as const) {
    vi.spyOn(Storage.prototype, method).mockImplementation(() => { throw new Error('storage unavailable'); });
  }
  const { createPilotSession } = await import('./pilotSessionCreation');
  vi.mocked(api).mockRejectedValueOnce(new Error('response lost'));
  await expect(createPilotSession()).rejects.toThrow('response lost');
  const headers = vi.mocked(api).mock.calls[0][4];
  vi.mocked(api).mockResolvedValue({ session: { id: 'retained' } });
  await createPilotSession();
  expect(vi.mocked(api).mock.calls[1][4]).toEqual(headers);
  await createPilotSession();
  expect(vi.mocked(api).mock.calls[2][4]).not.toEqual(headers);
});

it('does not clear a newer creation key when an older concurrent response arrives', async () => {
  const { createPilotSession } = await import('./pilotSessionCreation');
  const releases: ((value: { session: { id: string } }) => void)[] = [];
  vi.mocked(api).mockImplementation(() => new Promise(resolve => { releases.push(resolve); }));
  const first = createPilotSession(), duplicate = createPilotSession();
  await vi.waitFor(() => expect(api).toHaveBeenCalledTimes(2));
  const firstHeaders = vi.mocked(api).mock.calls[0][4];
  expect(vi.mocked(api).mock.calls[1][4]).toEqual(firstHeaders);
  releases[0]({ session: { id: 'first' } });
  await first;
  const next = createPilotSession();
  await vi.waitFor(() => expect(api).toHaveBeenCalledTimes(3));
  const nextHeaders = vi.mocked(api).mock.calls[2][4];
  expect(nextHeaders).not.toEqual(firstHeaders);
  releases[1]({ session: { id: 'first' } });
  await duplicate;
  expect(localStorage.getItem(localStorage.key(0)!)).toBe(nextHeaders?.['Idempotency-Key']);
  releases[2]({ session: { id: 'next' } });
  await next;
  expect(localStorage.length).toBe(0);
});
