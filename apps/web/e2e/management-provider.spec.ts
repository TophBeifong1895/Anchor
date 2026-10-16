import { expect, test } from '@playwright/test';
import type { ChildProcess } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { gooseEnvironment, nativeHost, nativeHostBinary, repo, stop } from './fixtures/native-host';

test('WebUI hot-updates the next Pilot model and preserves native history and tools across restart', async ({ page, request }) => {
  test.skip(process.env.ANCHOR_REAL_PROVIDER !== '1', 'requires explicit real provider opt-in');
  test.setTimeout(300000);
  const endpoint = process.env.ANCHOR_MODEL_URL!;
  const model = process.env.ANCHOR_MODEL_NAME!;
  // An optional second model must be a known available model on this same service.
  // With one model, this verifies continuity without claiming a distinct model switch.
  const secondModel = process.env.ANCHOR_TEST_SECOND_MODEL || model;
  const key = process.env.ANCHOR_MODEL_API_KEY!;
  if (!endpoint || !model || !key) throw new Error('real provider configuration is incomplete');
  const root = await mkdtemp(join(tmpdir(), 'anchor-management-provider-'));
  const artifacts = join(process.env.ANCHOR_TEST_EVIDENCE_ROOT || join(repo, '.local/model-execution-20261015/provider-real'), basename(root));
  await mkdir(artifacts, { recursive: true });
  const hostSha256 = createHash('sha256').update(await readFile(nativeHostBinary())).digest('hex');
  // Legacy deployment fields seed default.flash until the default slot is saved.
  const host = await nativeHost(root, { environment: {
    ...await gooseEnvironment(), ANCHOR_MODEL_URL: endpoint, ANCHOR_MODEL_NAME: model,
    ANCHOR_MODEL_API_KEY: key, ANCHOR_MODEL_WIRE_API: process.env.ANCHOR_MODEL_WIRE_API || 'chat',
    ...(process.env.ANCHOR_MODEL_CONTEXT_WINDOW ? { ANCHOR_MODEL_CONTEXT_WINDOW: process.env.ANCHOR_MODEL_CONTEXT_WINDOW } : {}),
  } });
  let backend: ChildProcess | undefined;
  const read = async (path: string) => {
    const response = await request.get(host.base + path);
    expect(response.status()).toBe(200);
    const body = await response.text();
    expect(body.includes(key)).toBe(false);
    return JSON.parse(body);
  };
  const ready = async () => {
    await expect.poll(async () => {
      try { return (await request.get(`${host.base}/health`)).status(); } catch { return 0; }
    }, { timeout: 20000 }).toBe(200);
  };
  const waitTurn = async (session: string, count: number) => {
    await expect.poll(async () => {
      const { turns } = await read(`/sessions/${session}/turns`);
      if (turns.length < count) return 'pending';
      return turns[0]?.status;
    }, { timeout: 120000, intervals: [500, 1000, 2000] }).toBe('completed');
  };
  try {
    backend = host.start(); await ready();
    const original = await read('/settings/model');
    expect(original.execution).toMatchObject({ default_reference: 'models.default.flash', source: 'legacy', configured: true, fields: { model } });
    await page.addInitScript(() => localStorage.setItem('anchor:view', 'settings'));
    await page.goto(host.base);
    for (const slot of ['default', 'backup'] as const) {
      const group = page.getByTestId(`model-provider-${slot}`);
      await group.getByLabel('服务地址', { exact: true }).fill(endpoint);
      await group.getByRole('combobox', { name: '接口协议', exact: true }).selectOption(process.env.ANCHOR_MODEL_WIRE_API || 'chat');
      await group.getByLabel('Flash 模型', { exact: true }).fill(model);
      await group.getByLabel('Pro 模型', { exact: true }).fill(model);
      if (slot === 'backup') {
        // Avoid Playwright fill logs containing a live secret if an input action fails.
        await group.getByLabel('模型服务 API key', { exact: true }).evaluate((element, value) => {
          (element as HTMLInputElement).value = value;
          element.dispatchEvent(new Event('input', { bubbles: true }));
        }, key);
      }
      const before = await read('/settings/model');
      const [probe] = await Promise.all([
        page.waitForResponse(response => response.url().endsWith(`/settings/model/providers/${slot}/test`) && response.request().method() === 'POST', { timeout: 30000 }),
        group.getByRole('button', { name: slot === 'default' ? '测试 Flash' : '测试 Pro', exact: true }).click(),
      ]);
      const probeResult = (await probe.text()).split(key).join('[REDACTED]');
      expect(probe.status(), probeResult).toBe(200);
      await expect(group.getByRole('status')).toContainText('通过');
      expect(await read('/settings/model')).toEqual(before);
      await group.getByRole('button', { name: '保存配置', exact: true }).click();
      await expect(group.getByRole('status')).toContainText('已保存');
      await expect(group.getByLabel('模型服务 API key', { exact: true })).toHaveValue('');
    }
    const configured = await read('/settings/model');
    // Legacy compatibility data remains readable; execution is the effective next default.
    expect(configured.fields).toEqual(original.fields);
    expect(configured.execution).toMatchObject({ default_reference: 'models.default.flash', source: 'host', configured: true, api_key_present: true, fields: { endpoint, wire_api: process.env.ANCHOR_MODEL_WIRE_API || 'chat', model }, error: null });
    expect(configured.providers.default.configured).toBe(true);
    expect(configured.providers.backup.configured).toBe(true);
    const browserState = await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } }));
    expect(browserState.includes(key)).toBe(false);
    await page.screenshot({ path: join(artifacts, 'settings.png'), fullPage: true });
    await page.getByRole('button', { name: 'Pilot', exact: true }).click();
    const input = page.getByLabel('发送给 Anchor Pilot');
    await input.fill('请心算 17 + 25，直接回复数字即可，不要调用工具。'); await input.press('Enter');
    await expect.poll(async () => (await read('/sessions')).sessions.length).toBe(1);
    const session = (await read('/sessions')).sessions[0].id;
    await waitTurn(session, 1);
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('42');
    const firstTurn = (await read(`/sessions/${session}/turns`)).turns[0];
    expect(firstTurn.goose.session).toBeTruthy();
    // Update the next default through the settings form after the first turn is complete.
    await page.getByRole('button', { name: '设置', exact: true }).click();
    const defaultGroup = page.getByTestId('model-provider-default');
    await defaultGroup.getByLabel('Flash 模型', { exact: true }).fill(secondModel);
    await defaultGroup.getByRole('button', { name: '保存配置', exact: true }).click();
    await expect(defaultGroup.getByRole('status')).toContainText('下一轮使用新设置');
    const updated = await read('/settings/model');
    expect(updated.execution).toMatchObject({ default_reference: 'models.default.flash', source: 'host', configured: true, fields: { model: secondModel } });
    // The second turn uses the saved default while the same Host process is still running.
    await page.getByRole('button', { name: 'Pilot', exact: true }).click();
    await expect(input).toBeEnabled();
    await input.fill('把你上一条回答的数字加一，只回复结果，不要调用工具。'); await input.press('Enter');
    await waitTurn(session, 2);
    expect((await read('/sessions')).sessions.length).toBe(1);
    const secondTurn = (await read(`/sessions/${session}/turns`)).turns[0];
    expect(secondTurn.goose.session).toBe(firstTurn.goose.session);
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('43');
    await page.screenshot({ path: join(artifacts, 'pilot-hot-update.png'), fullPage: true });

    await stop(backend); backend = host.start(); await ready();
    const restored = await read('/settings/model');
    expect(restored.execution).toEqual(updated.execution);
    expect(restored.providers.default.fields.flash).toBe(secondModel);
    await page.reload();
    await page.getByRole('button', { name: 'Pilot', exact: true }).click();
    await expect(input).toBeEnabled();
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('43');
    await input.fill('先调用 graph_list 列工作流，再把上条数字加一；只回复结果及工作流数量，不要运行工作流。'); await input.press('Enter');
    await waitTurn(session, 3);
    expect((await read('/sessions')).sessions.length).toBe(1);
    const completed = (await read(`/sessions/${session}/turns`)).turns;
    expect(completed).toHaveLength(3);
    expect(completed[0].goose.session).toBe(firstTurn.goose.session);
    const history = await read(`/sessions/${session}/messages`);
    expect(history.messages.filter((message: { role: string }) => message.role === 'assistant').length).toBeGreaterThanOrEqual(3);
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('44');
    const events = await request.get(`${host.base}/sessions/${session}/turns/${completed[0].id}/events`);
    expect(events.status()).toBe(200);
    const eventText = (await events.text()).split(key).join('[REDACTED]');
    const chunks: { type?: string; toolName?: string; toolCallId?: string }[] = eventText.split('\n').filter(line => line.startsWith('data: ')).flatMap(line => {
      try { return [JSON.parse(line.slice(6))]; } catch { return []; }
    });
    const graphList = chunks.find(chunk => chunk.type === 'tool-input-available' && chunk.toolName?.endsWith('graph_list'));
    expect(graphList?.toolCallId).toBeTruthy();
    expect(chunks.some(chunk => chunk.type === 'tool-output-available' && chunk.toolCallId === graphList!.toolCallId)).toBe(true);
    await writeFile(join(artifacts, 'third-turn-events.txt'), eventText);
    await page.screenshot({ path: join(artifacts, 'pilot.png'), fullPage: true });
    await writeFile(join(artifacts, 'evidence.json'), JSON.stringify({ status: 'passed', actual_host: true,
      actual_goose: true, host_sha256: hostSha256, provider: 'real', initial_model: model, updated_model: secondModel,
      distinct_model_names: secondModel !== model, session, native_session: firstTurn.goose.session, completed_turns: 3,
      deployment_configuration_present: true, two_provider_slots_saved: true, draft_test_did_not_save: true,
      next_default_projection_after_save: updated.execution, next_default_projection_after_restart: restored.execution,
      hot_update_without_restart: { completed_turn: secondTurn.id, same_native_session: true, observed_answer: '43' },
      restart_history_and_tools: { completed_turn: completed[0].id, same_native_session: true, observed_answer: '44', graph_list_completed: true },
      browser_secret_storage: false, active_turn_snapshot_verified: false, explicit_graph_model_routes_verified: false,
      scope: 'Known real model parameters, a second turn immediately after settings save, then a third turn with graph_list after restart, all in the same native Session; exact provider request routing is verified separately by deterministic Provider tests' }, null, 2));
    console.log(`Real WebUI model evidence: ${artifacts}`);
  } finally {
    await stop(backend);
    await writeFile(join(artifacts, 'host.log'), host.logs.split(key).join('[REDACTED]'));
    // The fixture is disposable; remove its live provider credential even after failure.
    await rm(join(root, 'state/managed/model-config.json'), { force: true });
  }
});
