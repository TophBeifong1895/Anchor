import { test, expect } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  const authorized = (request: { headers(): Record<string, string> }) =>
    request.headers().authorization === 'Bearer test-anchor-api-key';
  await page.route('**/graphs', route => authorized(route.request())
    ? route.fulfill({ json: { graphs: [{ graph: 'test-graph', running: null }] } })
    : route.fulfill({ status: 401, json: { error: 'unauthorized' } }));
  await page.route('**/runs', route => authorized(route.request())
    ? route.fulfill({ json: { runs: [] } })
    : route.fulfill({ status: 401, json: { error: 'unauthorized' } }));
  await page.route('**/timeline*', route => authorized(route.request())
    ? route.fulfill({ json: { runs: [], scheduled: [], schedules: [] } })
    : route.fulfill({ status: 401, json: { error: 'unauthorized' } }));
  await page.route('**/graphs/test-graph', route => authorized(route.request())
    ? route.fulfill({ json: { definition: { entry: 'assistant', agents: { assistant: {} }, nodes: [], edges: [] } } })
    : route.fulfill({ status: 401, json: { error: 'unauthorized' } }));
});

test('asks for the Anchor API key in the page and retries protected requests', async ({ page }) => {
  await page.goto('/');
  const dialog = page.getByRole('dialog', { name: '连接 Anchor 服务' });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel('Anchor API key').fill('test-anchor-api-key');
  await dialog.getByRole('button', { name: '连接' }).click();
  await expect(dialog).not.toBeVisible();
  await expect(page.getByLabel('当前工作流')).toHaveValue('test-graph');
  await expect(page.getByText('后端请求失败', { exact: false })).toHaveCount(0);
});

test('recovers an invalid cached key and keeps wrong or unsafe drafts in the dialog', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => sessionStorage.setItem('anchor-api-key', '错误的中文密钥'));
  await page.goto('/');
  const dialog = page.getByRole('dialog', { name: '连接 Anchor 服务' });
  await expect(dialog).toBeVisible();
  expect(await page.evaluate(() => sessionStorage.getItem('anchor-api-key'))).toBeNull();

  await dialog.getByLabel('Anchor API key').fill('wrong-ascii-key');
  await dialog.getByRole('button', { name: '连接' }).click();
  await expect(dialog.getByRole('alert')).toHaveText('Anchor API key 不正确，请检查后重新输入。');
  await expect(dialog).toBeVisible();
  expect(await page.evaluate(() => sessionStorage.getItem('anchor-api-key'))).toBeNull();

  await dialog.getByLabel('Anchor API key').fill('中文密钥');
  await dialog.getByRole('button', { name: '连接' }).click();
  await expect(dialog.getByRole('alert')).toHaveText('Anchor API key 格式不正确，请检查是否混入中文、空格或换行。');
  await expect(dialog).toBeVisible();
  expect(await page.evaluate(() => sessionStorage.getItem('anchor-api-key'))).toBeNull();

  await dialog.getByLabel('Anchor API key').fill('  test-anchor-api-key  ');
  await dialog.getByRole('button', { name: '连接' }).click();
  await expect(dialog).not.toBeVisible();
  await expect(page.getByLabel('当前工作流')).toHaveValue('test-graph');
  await expect(page.getByText('后端 API 暂不可达', { exact: false })).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('cancelling key verification does not save a late successful response', async ({ page }) => {
  let release: (() => void) | undefined;
  let validating: (() => void) | undefined;
  const begun = new Promise<void>(resolve => { validating = resolve; });
  const delayed = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/graphs', async route => {
    if (route.request().headers().authorization !== 'Bearer test-anchor-api-key') {
      await route.fulfill({ status: 401, json: { error: 'unauthorized' } });
      return;
    }
    validating!();
    await delayed;
    await route.fulfill({ json: { graphs: [] } }).catch(() => undefined);
  });
  await page.goto('/');
  const dialog = page.getByRole('dialog', { name: '连接 Anchor 服务' });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel('Anchor API key').fill('test-anchor-api-key');
  await dialog.getByRole('button', { name: '连接' }).click();
  await begun;
  await dialog.getByRole('button', { name: '取消' }).click();
  release!();
  await expect(dialog).not.toBeVisible();
  expect(await page.evaluate(() => sessionStorage.getItem('anchor-api-key'))).toBeNull();
});
