import { expect, test } from '@playwright/test';
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { freePort } from './fixtures/native-host';
import { loopbackProvider, toolResultField } from './fixtures/loopback-provider';

const execute = promisify(execFile);

test('source launcher prepares its runtime and WebUI configures Pilot and AgentNode', async ({ page, request }, info) => {
  const configuredCheckout = process.env.ANCHOR_TEST_SOURCE_CHECKOUT;
  test.skip(!configuredCheckout, 'requires an isolated source checkout; never operates on the running development service');
  test.setTimeout(600000);
  const checkout = resolve(configuredCheckout!);
  const root = await mkdtemp(join(tmpdir(), 'anchor-source-browser-'));
  const apiPort = await freePort(), webPort = await freePort();
  const base = `http://127.0.0.1:${apiPort}`, web = `http://127.0.0.1:${webPort}`;
  const environment = { PATH: process.env.PATH || '/usr/bin:/bin', HOME: process.env.HOME,
    ...(process.env.CARGO_TARGET_DIR ? { CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR } : {}),
    ANCHOR_DEV_ROOT: join(root, 'data'), ANCHOR_RUNNER_LISTEN: `127.0.0.1:${apiPort}`, ANCHOR_WEB_PORT: String(webPort) };
  let logs = '';
  const command = async (action: string) => {
    const result = await execute(join(checkout, 'scripts/dev.sh'), [action], { cwd: checkout, env: environment,
      timeout: 480000, maxBuffer: 8 * 1024 * 1024 });
    logs += result.stdout + result.stderr;
  };
  const problems: string[] = [];
  page.on('pageerror', error => problems.push(error.message));
  const provider = await loopbackProvider((body, position) => {
    if (position === 1) return { tool: { name: 'graph_read', arguments: { graph: 'source-check' } } };
    if (position === 2) {
      expect(toolResultField(body, 'objective')).toBe('source-check-marker');
      return { text: 'source-pilot-tool-marker' };
    }
    if (position === 3) return { tool: { name: 'anchor_run', arguments: {
      command: ['sh', '-c', "printf 'source-agent-artifact' > result.txt"],
    } } };
    if (position === 4) {
      const receipt = toolResultField(body, 'anchor_receipt');
      expect(typeof receipt).toBe('string');
      return { tool: { name: 'final_result', arguments: { summary: 'source agent completed', observed_receipt: receipt } } };
    }
    if (position === 5) {
      expect(JSON.stringify(body.messages)).toContain('source-pilot-tool-marker');
      return { text: 'source-restart-history-marker' };
    }
    throw new Error('unexpected Provider request');
  });
  try {
    await command('start');
    const goosePath = join(checkout, '.local/bin/goose');
    const gooseDigest = createHash('sha256').update(await readFile(goosePath)).digest('hex');
    expect((await request.get(`${base}/health`)).ok()).toBeTruthy();
    expect((await request.get(`${base}/ready`)).ok()).toBeTruthy();
    expect((await (await request.get(`${base}/settings/model`)).json()).execution.configured).toBe(false);
    await page.addInitScript(() => localStorage.setItem('anchor:view', 'pilot'));
    await page.goto(web);
    const composer = page.getByLabel('发送给 Anchor Pilot');
    await expect(composer).toBeEnabled();
    await composer.fill('Read the source-check Graph without running it.');
    await composer.press('Enter');
    await expect(page.locator('.pilot-chat').getByRole('alert')).toContainText('请先在「设置 → 模型服务」配置默认服务');
    await page.getByRole('button', { name: '前往模型设置', exact: true }).click();
    const form = page.getByTestId('model-provider-default');
    await form.getByLabel('服务地址').fill(provider.url);
    await form.getByLabel('接口协议').selectOption('chat');
    await form.getByLabel('Flash 模型', { exact: true }).fill('fixture-browser');
    await form.getByLabel('模型服务 API key').fill('fixture-source-web-key');
    await form.getByRole('button', { name: '保存配置', exact: true }).click();
    await expect(form.getByRole('status')).toContainText('配置已保存');
    const created = await request.post(`${base}/graphs`, { data: { name: 'source-check', definition: {
      objective: 'source-check-marker', entry: 'build', agents: { writer: { model: 'models.default.flash',
        instructions: 'Write result.txt and complete the node.', network: false, writes: ['result.txt'] } },
      nodes: [{ id: 'build', agent: 'writer' }], edges: [],
    } } });
    expect(created.ok(), await created.text()).toBeTruthy();
    await page.getByRole('button', { name: 'Pilot', exact: true }).click();
    await expect(composer).toHaveValue('Read the source-check Graph without running it.');
    await composer.press('Enter');
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('source-pilot-tool-marker', { timeout: 30000 });
    const triggered = await request.post(`${base}/trigger`, { data: { graph: 'source-check' } });
    expect(triggered.ok(), await triggered.text()).toBeTruthy();
    const run = (await triggered.json()).run;
    await expect.poll(async () => (await (await request.get(`${base}/runs`)).json()).runs.find((row: { run: string }) => row.run === run)?.status,
      { timeout: 30000 }).toBe('completed');
    const artifact = await request.get(`${base}/runs/${run}/files/build/result.txt?download=1`);
    expect(artifact.ok()).toBeTruthy(); expect(await artifact.text()).toBe('source-agent-artifact');
    await command('restart');
    expect(createHash('sha256').update(await readFile(goosePath)).digest('hex')).toBe(gooseDigest);
    await page.reload();
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('source-pilot-tool-marker');
    await expect(composer).toBeEnabled();
    await composer.fill('Recall the saved result.'); await composer.press('Enter');
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('source-restart-history-marker', { timeout: 30000 });
    const browserState = await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } }));
    expect(browserState).not.toContain('fixture-source-web-key');
    expect(provider.failures).toEqual([]); expect(provider.calls).toHaveLength(5); expect(problems).toEqual([]);
    await page.screenshot({ path: info.outputPath('source-configured-pilot.png'), fullPage: true });
    await writeFile(info.outputPath('evidence.json'), JSON.stringify({ status: 'passed', checkout, root,
      goose_binary_sha256: gooseDigest, restart_reused_local_build: true, actual_source_launcher: true, actual_host: true, actual_goose: true, real_model_calls: 0,
      runtime_path_overrides: false, model_configuration: 'WebUI', missing_model_hint: true,
      pilot_authorized_tool: true, agent_node_artifact: true, restart_history_preserved: true,
      provider_requests: provider.calls.length, run }, null, 2));
  } finally {
    try { await command('stop'); } finally {
      await provider.close(); await mkdir(info.outputDir, { recursive: true });
      await writeFile(info.outputPath('launcher.log'), logs);
      await writeFile(info.outputPath('host.log'), await readFile(join(root, 'data/dev/native-host.log'), 'utf8').catch(() => 'unavailable'));
    }
  }
});
