import { expect, test, type APIRequestContext, type Page, type TestInfo } from '@playwright/test';
import type { ChildProcess } from 'node:child_process';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { nativeHost, repo, stop } from './fixtures/native-host';

const graphName = 'management-graph';
type Host = Awaited<ReturnType<typeof nativeHost>>;
const environment = { };

async function ready(host: Host, request: APIRequestContext) {
  await expect(async () => {
    expect((await request.get(`${host.base}/health`)).status(), host.logs).toBe(200);
  }).toPass({ timeout: 20000 });
}
async function open(page: Page, host: Host, view: string) {
  await page.addInitScript(view => {
    localStorage.setItem('anchor:view', view);
  }, view);
  await page.goto(host.base);
}
async function screenshot(page: Page, root: string, name: string, info: TestInfo) {
  const directory = await artifacts(root);
  const path = join(directory, `${name}.png`);
  await page.screenshot({ path, fullPage: true });
  await info.attach(name, { path, contentType: 'image/png' });
}
async function evidence(root: string, facts: Record<string, unknown>, info: TestInfo) {
  const directory = await artifacts(root);
  const path = join(directory, 'evidence.json');
  await writeFile(path, JSON.stringify({ status: 'passed', host_fixture_root: root, actual_host: true, real_provider_calls: 0, ...facts }, null, 2));
  await info.attach('native-management-evidence', { path, contentType: 'application/json' });
  console.log(`Native management evidence: ${directory}`);
}
async function artifacts(root: string) {
  const directory = join(repo, '.local/webui-lifecycle-20261014/native-management-e2e', basename(root));
  await mkdir(directory, { recursive: true });
  return directory;
}
async function hostLog(root: string, host: Host) {
  await writeFile(join(await artifacts(root), 'host.log'), host.logs);
}
async function noSecretInBrowser(page: Page, secret: string) {
  const persisted = await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage }, history: history.state }));
  expect(persisted).not.toContain(secret);
  await expect(page.locator('body')).not.toContainText(secret);
}

test('native plugin directory install, owner-bound private credentials and uninstall run without a model', async ({ page, request }, info) => {
  test.setTimeout(90000);
  const root = await mkdtemp(join(tmpdir(), 'anchor-management-plugin-'));
  const host = await nativeHost(root, { graphName, environment });
  const candidate = join(root, 'native-lifecycle-plugin');
  await mkdir(candidate, { recursive: true });
  await writeFile(join(candidate, 'plugin.json'), JSON.stringify({ name: '原生管理验收插件', description: 'Isolated browser lifecycle fixture',
    mcpServers: { api: { url: 'https://example.com/mcp', bearer_token_env_var: 'ANCHOR_NATIVE_MANAGEMENT_KEY' } } }));
  let backend: ChildProcess | undefined;
  const problems: string[] = []; page.on('pageerror', error => problems.push(error.message));
  const credentialsUrl = `${host.base}/plugins/native-lifecycle-plugin/credentials`;
  try {
    backend = host.start(); await ready(host, request);
    await open(page, host, 'plugins');
    await page.getByRole('button', { name: '安装或导入插件', exact: true }).click();
    const install = page.getByRole('dialog', { name: '安装或导入插件', exact: true });
    await install.getByLabel('插件标识', { exact: true }).fill('native-lifecycle-plugin');
    await install.getByLabel('插件目录', { exact: true }).setInputFiles(candidate);
    await install.getByRole('button', { name: '预检插件', exact: true }).click();
    await install.getByRole('checkbox', { name: '确认安装此摘要对应的插件', exact: true }).check({ timeout: 30000 });
    await expect(install).toContainText('ANCHOR_NATIVE_MANAGEMENT_KEY');
    await install.getByRole('button', { name: '确认安装插件', exact: true }).click();
    await expect(install).not.toBeVisible({ timeout: 30000 });
    const card = page.locator('.plugin-card').filter({ has: page.getByRole('heading', { name: '原生管理验收插件', exact: true }) });
    await expect(card).toContainText('已安装');
    await card.getByRole('button', { name: '查看 原生管理验收插件 的详情', exact: true }).click();
    const details = page.getByRole('dialog').filter({ has: page.getByRole('heading', { name: '原生管理验收插件 · 插件详情', exact: true }) });
    await details.getByRole('button', { name: '设置 ANCHOR_NATIVE_MANAGEMENT_KEY', exact: true }).click();
    const input = details.getByLabel('ANCHOR_NATIVE_MANAGEMENT_KEY 新值', { exact: true });
    await expect(input).toHaveAttribute('type', 'password');
    await input.fill('native-plugin-fixture-private-key');
    await details.getByRole('button', { name: '保存凭据', exact: true }).click();
    await expect(details.getByRole('status')).toContainText('已保存 ANCHOR_NATIVE_MANAGEMENT_KEY');
    const savedResponse = await request.get(credentialsUrl);
    expect(savedResponse.headers()['cache-control']).toContain('no-store');
    const saved = await savedResponse.json();
    expect(saved.targets[0].fields[0].state).toBe('configured');
    expect(JSON.stringify(saved)).not.toContain('native-plugin-fixture-private-key');
    await noSecretInBrowser(page, 'native-plugin-fixture-private-key');
    await screenshot(page, root, 'plugin-credentials-desktop', info);
    await page.reload();
    await card.getByRole('button', { name: '查看 原生管理验收插件 的详情', exact: true }).click();
    await expect(details.getByRole('button', { name: '轮换 ANCHOR_NATIVE_MANAGEMENT_KEY', exact: true })).toBeVisible();
    await details.getByRole('button', { name: '清除 ANCHOR_NATIVE_MANAGEMENT_KEY', exact: true }).click();
    await details.getByRole('button', { name: '确认清除', exact: true }).click();
    await expect(details.getByRole('status')).toContainText('已清除 ANCHOR_NATIVE_MANAGEMENT_KEY');
    expect((await (await request.get(credentialsUrl)).json()).targets[0].fields[0].state).toBe('missing');
    await details.getByRole('button', { name: '查看卸载影响', exact: true }).click();
    await details.getByRole('checkbox', { name: '确认卸载插件 native-lifecycle-plugin', exact: true }).check({ timeout: 30000 });
    await details.getByRole('checkbox', { name: '确认删除这个插件唯一的一份数据，不可恢复', exact: true }).check();
    await screenshot(page, root, 'plugin-uninstall-preflight', info);
    await details.getByRole('button', { name: '确认卸载', exact: true }).click();
    await expect(details).not.toBeVisible({ timeout: 30000 }); await expect(card).toHaveCount(0);
    const catalog = await (await request.get(`${host.base}/plugins`)).json();
    expect(catalog.plugins.some((plugin: { id: string }) => plugin.id === 'native-lifecycle-plugin')).toBe(false);
    expect((await (await request.get(`${host.base}/settings/model`)).json()).configured).toBe(false);
    expect(problems).toEqual([]);
    await evidence(root, { scenario: 'plugin-lifecycle', provider_requests: 0, install_source: 'uploaded fixture directory',
      private_credential_owner_isolated: true, credential_cleared: true, uninstall_completed: true,
      remote_mcp_invoked: false, model_configured: false }, info);
  } finally { await stop(backend); await hostLog(root, host); }
});

test('native run JSON, schedules and Graph or Run assistance preserve facts and never submit Pilot messages', async ({ page, request }, info) => {
  test.setTimeout(90000);
  const root = await mkdtemp(join(tmpdir(), 'anchor-management-schedule-run-'));
  const host = await nativeHost(root, { graphName, environment, definition: {
    objective: 'Native management browser acceptance', input: { topic: 'default-topic' }, entry: 'seed',
    ops: { seed: { run: "sh -c 'printf native-management > result.txt'" } }, nodes: [{ id: 'seed', op: 'seed' }], edges: [],
  } });
  let backend: ChildProcess | undefined;
  const problems: string[] = []; page.on('pageerror', error => problems.push(error.message));
  const submissions: string[] = [];
  page.on('request', message => { if (message.method() === 'POST' && /\/sessions\/[^/]+\/turns(?:\?|$)/.test(message.url())) submissions.push(message.url()); });
  try {
    backend = host.start(); await ready(host, request);
    await open(page, host, 'graph');
    await expect(page.locator('[data-id="seed"]')).toBeVisible();
    await page.getByRole('button', { name: '运行工作流', exact: true }).click();
    const inputDialog = page.getByRole('dialog', { name: `运行 ${graphName}`, exact: true });
    const input = inputDialog.getByRole('textbox', { name: '本次运行输入', exact: true });
    await expect(input).toHaveValue(JSON.stringify({ topic: 'default-topic' }, null, 2));
    await input.fill('[]'); await inputDialog.getByRole('button', { name: '开始运行', exact: true }).click();
    await expect(inputDialog.getByRole('alert')).toContainText('运行输入需要一个 JSON 对象');
    expect((await (await request.get(`${host.base}/runs`)).json()).runs).toHaveLength(0);
    const runInput = { topic: 'native-json-input', nested: { count: 3 }, tags: ['a', 'b'] };
    await input.fill(JSON.stringify(runInput)); await inputDialog.getByRole('button', { name: '格式化 JSON', exact: true }).click();
    await expect(input).toHaveValue(JSON.stringify(runInput, null, 2));
    const triggered = page.waitForResponse(value => value.url() === `${host.base}/trigger` && value.request().method() === 'POST');
    await inputDialog.getByRole('button', { name: '开始运行', exact: true }).click();
    const accepted = await triggered; expect(accepted.status()).toBe(202); const run = (await accepted.json()).run as string;
    await expect.poll(async () => (await (await request.get(`${host.base}/runs/${run}`)).json()).projection.outcome, { timeout: 30000 }).toBe('done');
    const detail = await (await request.get(`${host.base}/runs/${run}`)).json();
    expect(detail.state.input).toEqual(runInput);
    await expect(page.locator('.execution-node.state-completed')).toHaveCount(1);
    await screenshot(page, root, 'run-json-completed', info);
    await page.getByRole('button', { name: '在 Pilot 中诊断', exact: true }).click();
    await expect(page.getByLabel('发送给 Anchor Pilot', { exact: true })).toHaveValue(new RegExp(`#anchor/run/${run}`));
    expect(submissions).toHaveLength(0);
    await page.getByRole('button', { name: '图编排', exact: true }).click();
    await page.getByRole('button', { name: '在 Pilot 中协助', exact: true }).click();
    await expect(page.getByLabel('发送给 Anchor Pilot', { exact: true })).toHaveValue(new RegExp(`#anchor/graph/${graphName}`));
    expect(submissions).toHaveLength(0);
    await screenshot(page, root, 'pilot-reference-draft', info);
    expect((await (await request.get(`${host.base}/sessions`)).json()).sessions).toHaveLength(0);

    await page.getByRole('button', { name: '运行看板', exact: true }).click();
    await page.getByRole('button', { name: '添加计划', exact: true }).click();
    const plans = page.getByRole('dialog', { name: '定时计划', exact: true });
    await plans.getByRole('combobox', { name: '规则', exact: true }).selectOption('interval');
    await plans.getByLabel('间隔秒数', { exact: true }).fill('86400');
    await plans.getByRole('textbox', { name: '运行输入（可选 JSON）', exact: true }).fill('{"topic":"scheduled-topic"}');
    await plans.getByRole('button', { name: '保存计划', exact: true }).click();
    await expect(plans.getByRole('status')).toContainText('计划已创建');
    const list = async () => (await (await request.get(`${host.base}/schedules`)).json()).schedules;
    await expect.poll(async () => (await list()).length).toBe(1);
    const created = (await list())[0]; expect(created.rule).toEqual({ type: 'interval', seconds: 86400 });
    await plans.getByRole('button', { name: `编辑 ${graphName} 计划`, exact: true }).click();
    await plans.getByLabel('间隔秒数', { exact: true }).fill('172800');
    await plans.getByRole('textbox', { name: '运行输入（可选 JSON）', exact: true }).fill('{"topic":"edited-topic"}');
    await plans.getByRole('button', { name: '保存修改', exact: true }).click();
    await expect(plans.getByRole('status')).toContainText('计划已更新');
    await expect.poll(async () => (await list())[0].rule.seconds).toBe(172800);
    expect((await list())[0].input).toEqual({ topic: 'edited-topic' });
    await plans.getByRole('button', { name: '暂停', exact: true }).click();
    await expect.poll(async () => (await list())[0].enabled).toBe(false);
    await expect(plans.locator('.schedule-item')).toContainText('已暂停');
    await expect(plans.getByRole('checkbox', { name: '启用此计划', exact: true })).not.toBeChecked();
    await plans.getByRole('textbox', { name: '运行输入（可选 JSON）', exact: true }).fill('{"topic":"edited-while-paused"}');
    await plans.getByRole('button', { name: '保存修改', exact: true }).click();
    await expect(plans.getByRole('status')).toContainText('计划已更新');
    expect((await list())[0].enabled).toBe(false);
    expect((await list())[0].input).toEqual({ topic: 'edited-while-paused' });
    await plans.getByRole('button', { name: '启用', exact: true }).click();
    await expect.poll(async () => (await list())[0].enabled).toBe(true);
    expect(new Date((await list())[0].next_at).valueOf()).toBeGreaterThan(Date.now());
    await screenshot(page, root, 'schedule-edited-resumed', info);
    await plans.getByRole('button', { name: `删除 ${graphName} 计划`, exact: true }).click();
    await plans.getByRole('button', { name: '确认删除', exact: true }).click();
    await expect.poll(async () => (await list()).length).toBe(0);
    const retained = await (await request.get(`${host.base}/runs/${run}`)).json();
    expect(retained.projection.outcome).toBe('done'); expect(retained.state.input).toEqual(runInput);
    expect(submissions).toHaveLength(0); expect(problems).toEqual([]);
    await evidence(root, { scenario: 'schedules-run-assistance', provider_requests: 0, graph: graphName, run,
      runtime_path: 'native Host / Kernel / deterministic Op', schedule_created_edited_paused_resumed_deleted: true,
      manual_run_retained: true, json_input: runInput, pilot_reference_drafts_only: true, pilot_turn_submissions: 0 }, info);
  } finally { await stop(backend); await hostLog(root, host); }
});
