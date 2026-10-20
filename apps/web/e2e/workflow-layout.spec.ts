import { test, expect, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { runProjectionFixture } from './fixtures/runProjection';

async function geometry(page: Page) {
  await expect(page.locator('.workflow-canvas:visible')).toHaveAttribute('aria-busy', 'false');
  await expect(page.locator('.workflow-canvas:visible .react-flow__edge-path')).toHaveCount(12);
  return page.locator('.workflow-canvas:visible').evaluate(area => ({
    nodes: Array.from(area.querySelectorAll<HTMLElement>('.react-flow__node')).map(n => ({
      id: n.dataset.id, transform: n.style.transform, width: n.style.width, height: n.style.height,
    })),
    paths: Array.from(area.querySelectorAll('.react-flow__edge-path')).map(e => e.getAttribute('d')),
    ports: Array.from(area.querySelectorAll<HTMLElement>('.route-port')).map(p => [p.dataset.handleid, p.style.left, p.style.top]),
  }));
}

test('editor geometry survives edits while Run geometry stays frozen across view switches and polling', async ({ page }) => {
  let graph = JSON.parse(readFileSync('../../examples/graphs/deep-academic-research.json', 'utf8'));
  const frozenDefinition = structuredClone(graph);
  delete frozenDefinition.layout;
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(e.message));
  const run = { run: 'research-run', graph: 'research', status: 'running', running: true, projection: runProjectionFixture('running', { active: true }),
    started: '2026-09-24T08:00:00Z', updated: '2026-09-24T08:00:00Z', objective: '研究',
    executed: ['frame', 'investigate', 'challenge', 'feedback', 'frame', 'investigate'] };
  await page.route('**/graphs', r => r.fulfill({ json: { graphs: [{ graph: 'research', running: run.run }] } }));
  await page.route('**/graphs/research', async r => {
    if (r.request().method() === 'PUT') graph = r.request().postDataJSON().definition;
    await r.fulfill({ json: { definition: graph } });
  });
  let polls = 0;
  await page.route('**/runs', r => { polls++; return r.fulfill({ json: { runs: [run] } }); });
  await page.route('**/timeline*', r => r.fulfill({ json: { runs: [run], scheduled: [], schedules: [] } }));
  await page.route('**/runs/research-run', r => r.fulfill({ json: {
    graph: 'research', run: run.run, definition: frozenDefinition, projection: run.projection, nodes: frozenDefinition.nodes.map((n: { id: string }) => n.id), traces: {},
    state: { ...run, cursor: { node: 'investigate', pass: 2, dir: '' }, passes: { frame: 2, investigate: 2 },
      decided: { 'feedback|frame': [false, 2] }, nodes: {}, skipped: [], error: '' },
  } }));
  await page.goto('/');
  await expect(page.locator('.workflow-node')).toHaveCount(8);
  const initial = await geometry(page);
  await expect(page.locator('.workflow-edge-label')).toHaveCount(7);
  await page.screenshot({ path: 'test-results/layout-editor.png' });
  await page.getByRole('button', { name: '运行看板', exact: true }).click();
  await page.locator('.timeline-run').first().click();
  await page.getByRole('button', { name: '查看节点与产物', exact: true }).click();
  await expect(page.getByTestId('execution-canvas')).toBeVisible();
  const runGeometry = await geometry(page);
  expect(runGeometry.nodes.map(node => node.id)).toEqual(initial.nodes.map(node => node.id));
  const feedback = page.getByTestId('execution-canvas').getByTestId('edge-e3-feedback-frame').locator('.react-flow__edge-path');
  await expect(feedback).toHaveCSS('stroke-dasharray', 'none');
  await expect(feedback).toHaveCSS('stroke-width', '3.5px');
  await expect(page.getByTestId('execution-canvas').getByTestId('edge-e4-feedback-investigate').locator('.react-flow__edge-path')).not.toHaveCSS('stroke-dasharray', 'none');
  await page.screenshot({ path: 'test-results/layout-run.png' });
  await page.locator('.workflow-canvas:visible .react-flow__controls-zoomin').click();
  await page.waitForTimeout(250);
  const viewport = await page.locator('.workflow-canvas:visible .react-flow__viewport').getAttribute('style');
  const before = polls;
  await expect.poll(() => polls).toBeGreaterThan(before);
  expect(await geometry(page)).toEqual(runGeometry);
  expect(await page.locator('.workflow-canvas:visible .react-flow__viewport').getAttribute('style')).toBe(viewport);
  await page.getByRole('button', { name: '图编排', exact: true }).click();
  const node = page.locator('.react-flow__node[data-id="investigate"]');
  const box = (await node.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2 + 100, { steps: 8 });
  await page.mouse.up();
  await expect(page.getByText('● 未保存')).toBeVisible();
  const dragged = await geometry(page);
  expect(dragged.paths).not.toEqual(initial.paths);
  expect(dragged.nodes.find(n => n.id === 'investigate')).not.toEqual(initial.nodes.find(n => n.id === 'investigate'));
  await page.getByRole('button', { name: '运行看板', exact: true }).click();
  await page.locator('.timeline-run').first().click();
  await page.getByRole('button', { name: '查看节点与产物', exact: true }).click();
  expect(await geometry(page)).toEqual(runGeometry);
  await page.getByRole('button', { name: '图编排', exact: true }).click();
  await page.getByRole('button', { name: '自动整理', exact: true }).click();
  await expect.poll(() => geometry(page)).toEqual(initial);
  await page.getByRole('button', { name: '撤销', exact: true }).click();
  await expect.poll(() => geometry(page)).toEqual(dragged);
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByText('所有更改已保存')).toBeVisible();
  await page.reload();
  expect(await geometry(page)).toEqual(dragged);
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByTestId('graph-canvas')).toBeInViewport();
  await expect.poll(async () => page.locator('.workflow-canvas:visible .react-flow__edge-path').evaluateAll(paths => {
    const area = paths[0].closest('.workflow-canvas')!.getBoundingClientRect();
    return paths.every(p => { const b = p.getBoundingClientRect();
      return b.left >= area.left && b.right <= area.right && b.top >= area.top && b.bottom <= area.bottom;
    });
  })).toBe(true);
  expect(errors).toEqual([]);
});

test('returning to the editor reuses its canvas, viewport and search without a loading frame', async ({ page }) => {
  const graph = JSON.parse(readFileSync('../../examples/graphs/deep-academic-research.json', 'utf8'));
  await page.route('**/graphs', route => route.fulfill({ json: { graphs: [{ graph: 'research' }] } }));
  await page.route('**/graphs/research', route => route.fulfill({ json: { definition: graph } }));
  await page.route('**/runs', route => route.fulfill({ json: { runs: [] } }));
  await page.route('**/timeline*', route => route.fulfill({ json: { runs: [], scheduled: [], schedules: [] } }));
  await page.goto('/');
  const canvas = page.getByTestId('graph-canvas');
  await expect(canvas).toHaveAttribute('aria-busy', 'false');
  await expect(canvas.locator('.react-flow__node')).toHaveCount(8);
  const original = await canvas.elementHandle();
  const search = page.getByRole('textbox', { name: '搜索工作流' });
  await search.fill('research');
  const initialViewport = await canvas.locator('.react-flow__viewport').getAttribute('style');
  await canvas.locator('.react-flow__controls-zoomin').click();
  await expect(canvas.locator('.react-flow__viewport')).not.toHaveAttribute('style', initialViewport!);
  await expect.poll(() => canvas.locator('.react-flow__viewport').getAttribute('style')).toMatch(/scale\([^)]*\)/);
  const viewport = await canvas.locator('.react-flow__viewport').getAttribute('style');
  for (let index = 0; index < 3; index++) {
    await page.getByRole('button', { name: '运行看板', exact: true }).click();
    await expect(canvas).toBeHidden();
    await page.getByRole('button', { name: '图编排', exact: true }).click();
    await expect(canvas).toBeVisible();
    expect(await canvas.evaluate((element, previous) => element === previous, original)).toBe(true);
    await expect(canvas).toHaveAttribute('aria-busy', 'false');
    await expect(canvas.locator('.react-flow__node')).toHaveCount(8);
    await expect(canvas.locator('.react-flow__viewport')).toHaveAttribute('style', viewport!);
    await expect(search).toHaveValue('research');
    await expect(canvas.getByText('正在整理布局…')).toHaveCount(0);
  }
});
