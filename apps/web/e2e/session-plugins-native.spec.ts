import { expect, test } from '@playwright/test';
import type { ChildProcess } from 'node:child_process';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fixturePlugin, gooseEnvironment, nativeHost, stop } from './fixtures/native-host';
import { loopbackProvider, toolResultField } from './fixtures/loopback-provider';

test('native Library health stays visible and broken dependencies or definitions can be unmounted in the browser', async ({ page, request }) => {
  test.setTimeout(60000);
  const root = await mkdtemp(join(tmpdir(), 'anchor-broken-plugin-browser-'));
  const key = 'shared-plugin-browser-key-fixture-32';
  const headers = { Authorization: `Bearer ${key}` };
  const session = 'broken-plugin-session';
  const host = await nativeHost(root, { environment: { ANCHOR_API_KEYS: JSON.stringify([key]) } });
  const directory = await fixturePlugin(host.environment.ANCHOR_RUNNER_LIBRARY_ROOT!);
  const manifestPath = join(directory, 'plugin.json');
  const original = await readFile(manifestPath, 'utf8');
  let backend: ChildProcess | undefined;
  try {
    backend = host.start();
    await expect.poll(async () => {
      try { return (await request.get(`${host.base}/health`, { headers })).status(); } catch { return 0; }
    }, { timeout: 20000 }).toBe(200);
    expect((await request.post(`${host.base}/sessions`, { headers, data: { id: session } })).status()).toBe(201);
    await page.addInitScript(({ key, session }) => {
      sessionStorage.setItem('anchor-api-key', key);
      localStorage.setItem('anchor:view', 'pilot');
      localStorage.setItem('pilot:selected', session);
    }, { key, session });
    await page.goto(host.base);
    await page.getByRole('button', { name: '展开插件面板', exact: true }).click();
    const card = page.locator('.plugin-card').filter({ has: page.locator('.plugin-card-name', { hasText: '浏览器证据' }) });
    const switchPlugin = async (name: string, on: boolean) => {
      const saving = page.waitForResponse(response => response.url() === `${host.base}/sessions/${session}/plugins/browser-plugin`
        && response.request().method() === 'POST');
      await page.getByRole('switch', { name: `${on ? '挂载' : '取消挂载'} ${name}`, exact: true }).click();
      expect((await saving).status()).toBe(200);
    };
    await switchPlugin('浏览器证据', true);
    await writeFile(manifestPath, JSON.stringify({ ...JSON.parse(original), dependencies: { python: ['idna==3.7'] } }));
    await expect(card).toContainText('Plugin dependency view is missing or unusable', { timeout: 10000 });
    const row = (body: { plugins: { id: string; [key: string]: unknown }[] }) => body.plugins.find(plugin => plugin.id === 'browser-plugin')!;
    const catalog = row(await (await request.get(`${host.base}/plugins`, { headers })).json());
    const mounted = row(await (await request.get(`${host.base}/sessions/${session}/plugins`, { headers })).json());
    expect(catalog.definitionValid).toBe(true);
    expect(catalog.dependenciesReady).toBe(false);
    expect(catalog.available).toBe(false);
    expect(mounted.definitionValid).toBe(catalog.definitionValid);
    expect(mounted.dependenciesReady).toBe(catalog.dependenciesReady);
    expect(mounted.on).toBe(true);
    await switchPlugin('浏览器证据', false);
    await expect(page.getByRole('switch', { name: '挂载 浏览器证据', exact: true })).toBeDisabled();

    await writeFile(manifestPath, original);
    await expect(page.getByRole('switch', { name: '挂载 浏览器证据', exact: true })).toBeEnabled({ timeout: 10000 });
    await switchPlugin('浏览器证据', true);
    await writeFile(manifestPath, '{invalid manifest');
    const broken = page.locator('.plugin-card').filter({ has: page.locator('.plugin-card-name', { hasText: /^browser-plugin$/ }) });
    await expect(broken).toContainText('Plugin is unavailable or malformed', { timeout: 10000 });
    await expect(broken.getByRole('switch')).toHaveAttribute('aria-checked', 'true');
    await switchPlugin('browser-plugin', false);
    const revoked = row(await (await request.get(`${host.base}/sessions/${session}/plugins`, { headers })).json());
    expect(revoked.on).toBe(false);
    expect(revoked.definitionValid).toBe(false);
    expect(revoked.dependenciesReady).toBeNull();
    await page.screenshot({ path: test.info().outputPath('broken-plugin-revoked.png'), fullPage: true });
    await writeFile(test.info().outputPath('broken-plugin-evidence.json'), JSON.stringify({ actual_host: true,
      real_model_calls: 0, catalog, mounted, revoked, dependency_failure_revoked: true, invalid_manifest_revoked: true }, null, 2));
  } finally {
    await stop(backend);
    await writeFile(join(root, 'host.log'), host.logs);
    console.log(`Broken Plugin browser fixture: ${root}`);
  }
});

test('web mounting keeps the active Goose Turn frozen, continues once with latest mounts and respects stop', async ({ page, request }) => {
  test.skip(!process.env.ANCHOR_GOOSE_BINARY, 'pinned Goose binary required');
  test.setTimeout(90000);
  const root = await mkdtemp(join(tmpdir(), 'anchor-plugin-mount-browser-'));
  let releaseSource = () => {};
  let releaseStop = () => {};
  const sourceBoundary = new Promise<void>(resolve => { releaseSource = resolve; });
  const stopBoundary = new Promise<void>(resolve => { releaseStop = resolve; });
  const provider = await loopbackProvider(async (body, position) => {
    const instructions = JSON.stringify(body.messages.filter(message => message.role === 'system'));
    if (position === 1) {
      expect(instructions).not.toContain('browser-plugin');
      return { tool: { name: 'graph_read', arguments: { graph: 'mount-browser' } } };
    }
    if (position === 2) {
      expect(toolResultField(body, 'objective')).toBe('original-task-marker');
      await sourceBoundary;
      return { tool: { name: 'anchor_plugins', arguments: { query: 'browser-plugin' } } };
    }
    if (position === 3) {
      expect(toolResultField(body, 'plugins')).toEqual([]);
      expect(instructions).not.toContain('browser-plugin');
      return { text: 'Current capability snapshot remains frozen.' };
    }
    if (position === 4) {
      expect(instructions).toContain('browser-plugin');
      expect(JSON.stringify(body.messages)).toContain('original-task-marker');
      expect(JSON.stringify(body.messages)).toContain('Current capability snapshot remains frozen.');
      return { tool: { name: 'anchor_plugin_read', arguments: { plugin: 'browser-plugin', section: 'overview' } } };
    }
    if (position === 5) {
      expect(toolResultField(body, 'description')).toBe('Native Plugin browser fixture');
      return { tool: { name: 'anchor_plugin_read', arguments: {
        plugin: 'browser-plugin', section: 'resource', path: 'resources/input.txt',
      } } };
    }
    if (position === 6) {
      expect(toolResultField(body, 'text')).toBe('Native Plugin evidence');
      return { text: 'Continued the original task with Native Plugin evidence.' };
    }
    if (position === 7) {
      expect(instructions).toContain('browser-plugin');
      await stopBoundary;
      return { text: 'This response must not start a stopped continuation.' };
    }
    throw new Error(`unexpected Provider request ${position}`);
  });
  const key = 'plugin-mount-browser-operator-key-32';
  const headers = { Authorization: `Bearer ${key}` };
  const session = 'mount-browser-session';
  let backend: ChildProcess | undefined;
  const host = await nativeHost(root, { graphName: 'mount-browser',
    definition: { objective: 'original-task-marker', entry: 'idle', ops: { idle: { run: 'true' } },
      nodes: [{ id: 'idle', op: 'idle' }], edges: [] },
    environment: { ...await gooseEnvironment(provider.url), ANCHOR_API_KEYS: JSON.stringify([key]),
      ANCHOR_NODE_TOOL_DISCLOSURE: '1' },
  });
  await fixturePlugin(host.environment.ANCHOR_RUNNER_LIBRARY_ROOT!);
  const base = host.base;
  const turns = async () => (await (await request.get(`${base}/sessions/${session}/turns`, { headers })).json()).turns;
  const events = async () => (await (await request.get(`${base}/sessions/${session}/events`, { headers })).json()).events;
  const clickMount = async (on: boolean) => {
    const response = page.waitForResponse(value => value.url() === `${base}/sessions/${session}/plugins/browser-plugin`
      && value.request().method() === 'POST');
    await page.getByRole('switch', { name: `${on ? '挂载' : '取消挂载'} 浏览器证据`, exact: true }).click();
    const saved = await response;
    expect(saved.status()).toBe(200);
    return saved.json();
  };
  try {
    backend = host.start();
    await expect.poll(async () => {
      try { return (await request.get(`${base}/health`, { headers })).status(); } catch { return 0; }
    }).toBe(200);
    expect((await request.post(`${base}/sessions`, { headers, data: { id: session } })).status()).toBe(201);
    await page.addInitScript(({ key, session }) => {
      sessionStorage.setItem('anchor-api-key', key);
      localStorage.setItem('anchor:view', 'pilot');
      localStorage.setItem('pilot:selected', session);
    }, { key, session });
    await page.goto(base);
    const input = page.getByLabel('发送给 Anchor Pilot');
    await expect(input).toBeEnabled();
    await page.getByRole('button', { name: '展开插件面板', exact: true }).click();
    expect((await clickMount(true)).effect.status).toBe('next_turn');
    expect((await clickMount(false)).effect.status).toBe('next_turn');
    await expect(page.getByText('已保存，下次发消息时使用新设置')).toBeVisible();
    expect(provider.calls).toHaveLength(0);
    expect(await turns()).toHaveLength(0);

    await input.fill('Read the existing graph, then find the native evidence and finish the same task.');
    await input.press('Enter');
    await expect.poll(() => provider.calls.length).toBe(2);
    const source = (await turns())[0];
    for (const on of [true, false, true]) {
      const switched = await clickMount(on);
      expect(switched.effect).toEqual({ status: 'after_current_turn', source_turn: source.id });
      expect(switched.continuation).toEqual({ status: 'scheduled', request_id: `continuation:${source.id}` });
    }
    await expect(page.getByText('已保存。Pilot 会先完成当前回复，再使用新设置继续处理你的请求。')).toBeVisible();
    expect((await events()).filter((event: { kind: string }) => event.kind === 'pilot.continuation.requested')).toHaveLength(1);
    const snapshot = JSON.parse(await readFile(join(root, 'state/plugin-mounts', session, `${source.id}.json`), 'utf8'));
    expect(snapshot.mounts['browser-plugin']).toBe(false);
    releaseSource();
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('Continued the original task with Native Plugin evidence.');
    await expect(input).toBeEnabled();
    await expect(page.getByText('Pilot 已开始使用新设置继续处理你的请求。')).toBeVisible();
    const completed = await turns();
    expect(completed).toHaveLength(2);
    expect(completed[0].status).toBe('completed');
    expect(completed[0].request_id).toBe(`continuation:${source.id}`);
    expect(completed[0].goose.session).toBe(completed[1].goose.session);
    const fresh = JSON.parse(await readFile(join(root, 'state/plugin-mounts', session, `${completed[0].id}.json`), 'utf8'));
    expect(fresh.mounts['browser-plugin']).toBe(true);
    const continuationTrace = await (await request.get(`${base}/sessions/${session}/turns/${completed[0].id}/events`, { headers })).text();
    expect(continuationTrace).not.toContain('"toolName":"graph_read"');
    expect(provider.calls).toHaveLength(6);

    await input.fill('Inspect the same evidence again and wait for me.'); await input.press('Enter');
    await expect.poll(() => provider.calls.length).toBe(7);
    const stopped = (await turns())[0];
    expect((await clickMount(false)).effect.source_turn).toBe(stopped.id);
    await page.getByRole('button', { name: '停止', exact: true }).click();
    await expect.poll(async () => (await turns())[0].status).toBe('stopped');
    releaseStop();
    await expect(input).toBeEnabled();
    await expect(page.getByText('已保存。本次未继续，下次发消息时使用新设置。')).toBeVisible();
    await page.reload();
    await expect(input).toBeEnabled();
    expect(await turns()).toHaveLength(3);
    expect(provider.calls).toHaveLength(7);
    expect(provider.failures).toEqual([]);
    await writeFile(join(root, 'evidence.json'), JSON.stringify({ status: 'passed', provider: 'deterministic loopback',
      real_model_calls: 0, actual_host: true, actual_goose: true, session, source: source.id,
      continuation: completed[0].id, stopped: stopped.id, provider_requests: provider.calls.length,
      current_turn_frozen: true, idle_did_not_start: true, multiple_switches_merged: true, stop_prevented_continuation: true }));
    console.log(`Plugin mount browser evidence: ${root}`);
  } finally {
    releaseSource(); releaseStop();
    await stop(backend); await provider.close();
    await writeFile(join(root, 'host.log'), host.logs);
  }
});
