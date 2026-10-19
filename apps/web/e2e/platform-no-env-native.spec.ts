import { expect, test } from '@playwright/test';
import { createHash } from 'node:crypto';
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { freePort, repo, stop } from './fixtures/native-host';
import { loopbackProvider, toolResultField } from './fixtures/loopback-provider';

test('a fresh installed platform configures and runs from WebUI without dotenv files', async ({ page, request }, info) => {
  const archive = process.env.ANCHOR_TEST_PLATFORM_ARCHIVE;
  test.skip(!archive, 'requires a freshly built format-2 platform archive');
  test.setTimeout(120000);
  const root = await mkdtemp(join(tmpdir(), 'anchor-platform-no-env-'));
  const installation = join(root, 'installation');
  const target = process.env.CARGO_TARGET_DIR || join(repo, 'rust/target');
  const installer = process.env.ANCHOR_TEST_PLATFORM_INSTALLER || join(target, 'debug/anchor-platform');
  const devtools = process.env.ANCHOR_TEST_PLATFORM_DEVTOOLS || join(target, 'debug/anchor-devtools');
  const cleanEnvironment = { PATH: '/usr/bin:/bin', HOME: join(root, 'caller-home') };
  const digest = createHash('sha256').update(await readFile(archive!)).digest('hex');
  const installed = JSON.parse(execFileSync(installer, ['install', '--archive', archive!, '--sha256', digest,
    '--install-root', installation], { cwd: root, env: cleanEnvironment, encoding: 'utf8' }));
  const preflight = () => JSON.parse(execFileSync(devtools, ['preflight', '--launcher', installed.launcher],
    { cwd: root, env: cleanEnvironment, encoding: 'utf8' }));
  expect(preflight()).toEqual({ status: 'passed' });
  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;
  let backend: ChildProcess | undefined;
  let logs = '';
  const problems: string[] = [];
  page.on('pageerror', error => problems.push(error.message));
  const start = () => {
    const child = spawn(installed.launcher, ['start', '--listen', `127.0.0.1:${port}`], {
      cwd: root, env: cleanEnvironment, stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout!.on('data', chunk => { logs += chunk.toString(); });
    child.stderr!.on('data', chunk => { logs += chunk.toString(); });
    return child;
  };
  const ready = () => expect(async () => {
    expect((await request.get(`${base}/health`)).status(), logs).toBe(200);
    expect((await request.get(`${base}/ready`)).status(), logs).toBe(200);
  }).toPass({ timeout: 20000 });
  const provider = await loopbackProvider((body, position) => {
    if (position === 1) return { tool: { name: 'graph_read', arguments: { graph: 'no-env' } } };
    if (position === 2) {
      expect(toolResultField(body, 'objective')).toBe('no-env-graph-marker');
      return { text: 'no-env-tool-marker' };
    }
    if (position === 3) {
      expect(JSON.stringify(body.messages)).toContain('no-env-tool-marker');
      return { text: 'no-env-restart-marker' };
    }
    throw new Error('unexpected Provider request');
  });
  try {
    backend = start(); await ready();
    expect(provider.calls).toHaveLength(0);
    const before = await (await request.get(`${base}/settings/model`)).json();
    expect(before.execution.configured).toBe(false);
    await page.addInitScript(() => localStorage.setItem('anchor:view', 'pilot'));
    await page.goto(base);
    const input = page.getByLabel('发送给 Anchor Pilot');
    await expect(input).toBeEnabled();
    await input.fill('Read the no-env graph without running it.');
    await input.press('Enter');
    await expect(page.locator('.pilot-chat').getByRole('alert')).toContainText('请先在「设置 → 模型服务」配置默认服务');
    await page.getByRole('button', { name: '前往模型设置', exact: true }).click();
    const providerForm = page.getByTestId('model-provider-default');
    await providerForm.getByLabel('服务地址').fill(provider.url);
    await providerForm.getByLabel('接口协议').selectOption('chat');
    await providerForm.getByLabel('Flash 模型', { exact: true }).fill('fixture-browser');
    await providerForm.getByLabel('模型服务 API key').fill('fixture-platform-no-env-key');
    await providerForm.getByRole('button', { name: '保存配置', exact: true }).click();
    await expect(providerForm.getByRole('status')).toContainText('配置已保存');
    await expect(providerForm.getByLabel('Pro 模型', { exact: true })).toHaveValue('');
    await page.getByLabel('允许此次初始化联网下载官方解释器和工具链').check();
    await expect(page.getByRole('button', { name: '初始化 Python', exact: true })).toBeEnabled();
    await expect(page.getByRole('button', { name: '初始化 Rust', exact: true })).toBeEnabled();
    const created = await request.post(`${base}/graphs`, { data: { name: 'no-env', definition: {
      objective: 'no-env-graph-marker', entry: 'build', ops: { build: { run: "printf 'no-env-artifact' > result.txt" } },
      nodes: [{ id: 'build', op: 'build' }], edges: [],
    } } });
    expect(created.ok(), await created.text()).toBeTruthy();
    await page.getByRole('button', { name: 'Pilot', exact: true }).click();
    await expect(input).toHaveValue('Read the no-env graph without running it.');
    await expect(input).toBeEnabled(); await input.press('Enter');
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('no-env-tool-marker', { timeout: 30000 });
    const sessions = (await (await request.get(`${base}/sessions`)).json()).sessions;
    expect(sessions).toHaveLength(1);
    expect((await request.post(`${base}/trigger`, { data: { graph: 'no-env' } })).ok()).toBeTruthy();
    await expect.poll(async () => (await (await request.get(`${base}/runs`)).json()).runs[0]?.status,
      { timeout: 20000 }).toBe('completed');
    const run = (await (await request.get(`${base}/runs`)).json()).runs[0].run;
    const artifact = await request.get(`${base}/runs/${run}/files/build/result.txt?download=1`);
    expect(artifact.ok()).toBeTruthy(); expect(await artifact.text()).toBe('no-env-artifact');
    await stop(backend);
    const sentinel = join(root, 'dotenv-was-sourced');
    await writeFile(join(root, '.env'), `touch '${sentinel}'\nif\n`);
    await writeFile(join(installation, 'config/model.env'), 'invalid retired model configuration');
    expect(preflight()).toEqual({ status: 'passed' });
    backend = start(); await ready(); await page.reload();
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('no-env-tool-marker');
    await expect(input).toBeEnabled();
    await input.fill('Recall the saved result.'); await input.press('Enter');
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('no-env-restart-marker', { timeout: 30000 });
    await expect(readFile(sentinel)).rejects.toMatchObject({ code: 'ENOENT' });
    const browserState = await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } }));
    expect(browserState).not.toContain('fixture-platform-no-env-key');
    expect(provider.failures).toEqual([]); expect(provider.calls).toHaveLength(3); expect(problems).toEqual([]);
    await page.screenshot({ path: info.outputPath('configured-pilot.png'), fullPage: true });
    await writeFile(info.outputPath('evidence.json'), JSON.stringify({ status: 'passed', root, archive_sha256: digest,
      actual_host: true, actual_goose: true, actual_installed_launcher: true, real_model_calls: 0,
      model_configuration: 'WebUI', env_files_required: false, ignored_retired_files: true,
      python_bootstrap_available: true, rust_bootstrap_available: true,
      language_downloads_tested: false, provider_requests: provider.calls.length, session: sessions[0].id,
      run, restart_history_preserved: true, artifact_download: true }, null, 2));
  } finally {
    await stop(backend); await provider.close();
    await mkdir(info.outputDir, { recursive: true });
    await writeFile(info.outputPath('host.log'), logs);
  }
});
