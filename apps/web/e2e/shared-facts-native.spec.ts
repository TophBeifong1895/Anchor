import { expect, test, type APIRequestContext, type Page, type TestInfo } from '@playwright/test';
import type { ChildProcess } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { nativeHost, nativeHostBinary, repo, stop } from './fixtures/native-host';

const key = 'shared-facts-native-fixture-operator-32';
const headers = { Authorization: `Bearer ${key}` };
type Host = Awaited<ReturnType<typeof nativeHost>>;
async function ready(host: Host, request: APIRequestContext) {
  await expect(async () => {
    expect((await request.get(`${host.base}/health`, { headers })).status(), host.logs).toBe(200);
  }).toPass({ timeout: 20000 });
}
async function open(page: Page, host: Host) {
  await page.addInitScript(key => {
    sessionStorage.setItem('anchor-api-key', key);
    localStorage.setItem('anchor:view', 'graph');
  }, key);
  await page.goto(host.base);
}
async function directory(root: string) {
  const path = join(repo, '.local/shared-facts-fix-20261016/browser', basename(root));
  await mkdir(path, { recursive: true }); return path;
}
async function evidence(root: string, page: Page, info: TestInfo, facts: Record<string, unknown>) {
  const artifacts = await directory(root);
  const json = join(artifacts, 'evidence.json');
  await writeFile(json, JSON.stringify({ status: 'passed', actual_host: true, real_provider_calls: 0,
    host_sha256: createHash('sha256').update(await readFile(nativeHostBinary())).digest('hex'), ...facts }, null, 2));
  const screenshot = join(artifacts, 'browser.png');
  await page.screenshot({ path: screenshot, fullPage: true });
  await info.attach('shared-facts-evidence', { path: json, contentType: 'application/json' });
  await info.attach('shared-facts-browser', { path: screenshot, contentType: 'image/png' });
  console.log(`Shared facts native evidence: ${artifacts}`);
}

test('native Graph discovery excludes broken bundles and both role creation paths consume the Host default', async ({ page, request }, info) => {
  test.setTimeout(90000);
  const root = await mkdtemp(join(tmpdir(), 'anchor-shared-facts-authoring-'));
  const host = await nativeHost(root, { graphName: 'deployment-fixture', environment: { ANCHOR_API_KEYS: JSON.stringify([key]) } });
  const broken = join(root, 'catalog/broken-bundle');
  await mkdir(broken, { recursive: true });
  await writeFile(join(broken, 'graph.json'), JSON.stringify({ entry: 'missing', nodes: [], edges: [] }));
  await writeFile(join(broken, 'manifest.json'), JSON.stringify({ format: 1, graph: 'graph.json', plugins: [] }));
  let backend: ChildProcess | undefined;
  const problems: string[] = []; page.on('pageerror', error => problems.push(error.message));
  try {
    backend = host.start(); await ready(host, request);
    const provider = await request.put(`${host.base}/settings/model/providers/default`, { headers, data: {
      endpoint: 'https://example.invalid/v1', wire_api: 'chat', flash: 'fixture-default-flash', pro: 'fixture-default-pro',
      api_key: { action: 'set', value: 'fixture-only-no-provider-requests' },
    } });
    expect(provider.ok(), await provider.text()).toBeTruthy();
    const defaultReference = (await provider.json()).execution.default_reference as string;
    expect(defaultReference).toBe('models.default.flash');
    expect((await request.post(`${host.base}/graphs`, { headers, data: { name: 'authoring-role' } })).ok()).toBeTruthy();
    expect((await request.post(`${host.base}/graphs`, { headers, data: { name: 'authoring-parallel' } })).ok()).toBeTruthy();
    const listing = (await (await request.get(`${host.base}/graphs`, { headers })).json()).graphs;
    expect(listing.map((graph: { graph: string }) => graph.graph)).toEqual(['authoring-parallel', 'authoring-role', 'deployment-fixture']);
    expect((await request.get(`${host.base}/graphs/broken-bundle`, { headers })).ok()).toBe(false);
    await open(page, host);
    await expect(page.getByLabel('当前工作流').locator('option[value="broken-bundle"]')).toHaveCount(0);
    await page.getByLabel('当前工作流').selectOption('authoring-role');
    await page.getByRole('button', { name: '新增角色', exact: true }).click();
    await expect(page.getByLabel('模型', { exact: true })).toHaveValue(defaultReference);
    await page.getByRole('button', { name: '保存', exact: true }).click();
    await expect(page.getByText('已保存。', { exact: true })).toBeVisible();
    const role = (await (await request.get(`${host.base}/graphs/authoring-role`, { headers })).json()).definition.agents.agent;
    expect(role.model).toBe(defaultReference);
    await page.getByLabel('当前工作流').selectOption('authoring-parallel');
    await page.getByRole('button', { name: '添加节点', exact: true }).click();
    await page.getByRole('button', { name: '并行分支 在入口前' }).click();
    await expect(page.getByLabel('配对收束节点')).toHaveValue('join');
    await page.getByRole('button', { name: '保存', exact: true }).click();
    await expect(page.getByText('已保存。', { exact: true })).toBeVisible();
    const parallel = (await (await request.get(`${host.base}/graphs/authoring-parallel`, { headers })).json()).definition;
    expect(parallel.agents['parallel-agent'].model).toBe(defaultReference);
    expect(problems).toEqual([]);
    await evidence(root, page, info, { scenario: 'authoring-defaults-and-discovery', malformed_bundle_hidden: true,
      default_reference: defaultReference, role_saved: true, parallel_role_saved: true, provider_invoked: false });
  } finally {
    await stop(backend); await writeFile(join(await directory(root), 'host.log'), host.logs);
  }
});

test('native Run file download carries the tab Bearer key and produces the committed artifact', async ({ page, request }, info) => {
  test.setTimeout(90000);
  const root = await mkdtemp(join(tmpdir(), 'anchor-shared-facts-download-'));
  const content = 'shared-facts-download\n';
  const host = await nativeHost(root, { graphName: 'artifact-download', environment: { ANCHOR_API_KEYS: JSON.stringify([key]) },
    definition: { entry: 'write', ops: { write: { run: "printf 'shared-facts-download\\n' > evidence.txt", writes: ['evidence.txt'] } },
      nodes: [{ id: 'write', op: 'write' }], edges: [] } });
  let backend: ChildProcess | undefined;
  const problems: string[] = []; page.on('pageerror', error => problems.push(error.message));
  try {
    backend = host.start(); await ready(host, request); await open(page, host);
    await expect(page.locator('[data-id="write"]')).toBeVisible();
    const triggered = page.waitForResponse(response => response.url().endsWith('/trigger') && response.request().method() === 'POST');
    await page.getByRole('button', { name: '运行工作流', exact: true }).click();
    await page.getByRole('button', { name: '开始运行', exact: true }).click();
    const run = (await (await triggered).json()).run as string;
    await expect(page.locator('.execution-node.state-completed')).toHaveCount(1, { timeout: 20000 });
    await page.locator('[data-id="write"]').click();
    await page.getByRole('button', { name: '文件', exact: true }).click();
    await expect(page.getByRole('button', { name: '下载 evidence.txt', exact: true })).toBeVisible();
    const path = `/runs/${run}/files/write/evidence.txt?download=1`;
    expect((await request.get(host.base + path)).status()).toBe(401);
    const response = page.waitForResponse(value => value.url() === host.base + path);
    const downloading = page.waitForEvent('download');
    await page.getByRole('button', { name: '下载 evidence.txt', exact: true }).click();
    const downloaded = await downloading;
    expect(downloaded.suggestedFilename()).toBe('evidence.txt');
    expect(await readFile((await downloaded.path())!, 'utf8')).toBe(content);
    const received = await response;
    expect(received.status()).toBe(200);
    expect((await received.request().allHeaders()).authorization).toBe(`Bearer ${key}`);
    expect(problems).toEqual([]);
    await evidence(root, page, info, { scenario: 'authenticated-run-download', run,
      without_bearer_status: 401, browser_download_status: 200, downloaded_bytes: Buffer.byteLength(content), provider_invoked: false });
  } finally {
    await stop(backend); await writeFile(join(await directory(root), 'host.log'), host.logs);
  }
});
