# R2 Web 断言变更清单（S19–S26 候选，未批准）

直接变更的断言表达式共 30 处；参数化用例逐实例与 fixture 的间接预期变化另见下表。未删除用例、未增加 ignore。

| 编号 | 文件 | 测试名 | 原断言 | 新断言 | 行为分组 |
| --- | --- | --- | --- | --- | --- |
| W1 | `apps/web/e2e/goose-native-trace.spec.ts` | real Rust Host and Goose retain active trace, canonical completion and recorded Plugin metadata after refresh | `expect(during.state.status).toBe('running')` | `expect(during.projection?.phase).toBe('executing')` | S19/S20：Run 新词汇/worker 观察 |
| W2 | `apps/web/e2e/goose-native-trace.spec.ts` | real Rust Host and Goose retain active trace, canonical completion and recorded Plugin metadata after refresh | `expect.poll(async () => (await (await request.get(\`${base}/runs/${run}\`)).json()).state.status).toBe('completed')` | `expect.poll(async () => (await (await request.get(\`${base}/runs/${run}\`)).json()).projection.outcome).toBe('done')` | S19/S20：Run 新词汇/worker 观察 |
| W3 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect(detail.state.status).toBe('running')` | `expect(detail.projection?.phase).toBe('executing')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W4 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect(detail.calls[0].status).toBe('running')` | `expect(detail.calls[0].projection.phase).toBe('executing')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W5 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect(waitDetail.state.status).toBe('completed')` | `expect(waitDetail.projection?.outcome).toBe('done')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W6 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect(detail.state.status).toBe('completed')` | `expect(detail.projection?.outcome).toBe('done')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W7 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect(detail.calls[0].status).toBe('running')` | `expect(detail.calls[0].projection.phase).toBe('executing')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W8 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect((await json(request, \`${base}/runs/${detail.calls![0].run}\`)).state.status).toBe('completed')` | `expect((await json(request, \`${base}/runs/${detail.calls![0].run}\`)).projection.outcome).toBe('done')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W9 | `apps/web/e2e/graph-calls.spec.ts` | mocked: show wait/detach child Runs, source links and active controls | `expect(page.getByRole('button', { name: '暂停' })).toBeVisible()` | `expect(page.getByRole('button', { name: '放弃', exact: true })).toBeVisible()` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W10 | `apps/web/e2e/parallel-real.spec.ts` | real backend: paired authoring, simultaneous native nodes, joined artifacts and invalid-pair rejection | `expect(detail.state.status).toBe('completed')` | `expect(detail.projection?.outcome).toBe('done')` | S19/S20：Run 新词汇/worker 观察 |
| W11 | `apps/web/e2e/parallel.spec.ts` | mocked: show simultaneous branch execution and pending join in one Run | `expect(page.locator('.execution-node.state-running')).toHaveCount(2)` | `expect(page.locator('.execution-node.state-executing.is-live')).toHaveCount(2)` | S19/S20：Run 新词汇/worker 观察 |
| W12 | `apps/web/e2e/recovery.spec.ts` | unknown tool result resumes through the normal Agent context path | `expect(page.getByText('Agent 上次工具调用的结果未记录')).toBeVisible()` | `expect(page.locator('.canvas-head .pill', { hasText: '等待你处理：Agent 上次工具调用的结果未记录' })).toBeVisible()` | S19/S20：Run 新词汇/worker 观察 |
| W13 | `apps/web/e2e/recovery.spec.ts` | inactive running Run can be resumed from its existing detail page | `expect(page.locator('.execution-node.state-running')).toHaveCount(0)` | `expect(page.locator('.execution-node.is-live')).toHaveCount(0)` | S19/S20：Run 新词汇/worker 观察 |
| W14 | `apps/web/e2e/recovery.spec.ts` | inactive running Run can be resumed from its existing detail page | `expect(page.locator('.execution-node.state-waiting_resume')).toHaveCount(1)` | `expect(page.locator('.execution-node.state-executing')).toHaveCount(1)` | S19/S20：Run 新词汇/worker 观察 |
| W15 | `apps/web/e2e/recovery.spec.ts` | inactive running Run can be resumed from its existing detail page | `expect(page.getByText('宿主重启后尚未接续')).toBeVisible()` | `expect(page.locator('[aria-label="活动节点"]')).toContainText('等待接续')` | S19/S20：Run 新词汇/worker 观察 |
| W16 | `apps/web/src/ChannelsPage.test.tsx` | waits for the executor to exit before allowing retirement and binds it to the current Run | `expect(container.textContent).toContain('正在停止…')` | `expect(container.textContent).toContain('等待你处理：stopped')` | S19/S21：等待人/worker 排空/静止语义 |
| W17 | `apps/web/src/ChannelsPage.test.tsx` | does not infer resumability when controls are denied and uses human quiescence for retirement | `expect(button('退役实例').disabled).toBe(true)` | `expect(button('退役实例').disabled).toBe(false)` | S19/S21：等待人/worker 排空/静止语义 |
| W18 | `apps/web/src/RunDetailPage.test.tsx` | exposes the server-projected actions for %s | `expect(buttons.map(button => button.textContent)).toEqual(['停止', '继续'])` | `expect(buttons.map(button => button.textContent)).toEqual(['停止', '继续', '放弃'])` | S19/S22/S23/S24：统一等待文案/三动作 |
| W19 | `apps/web/src/RunDetailPage.test.tsx` | exposes the server-projected actions for %s | `expect(control.mock.calls).toEqual([['stop'], ['resume']])` | `expect(control.mock.calls).toEqual([['stop'], ['resume'], ['abandon']])` | S19/S22/S23/S24：统一等待文案/三动作 |
| W20 | `apps/web/src/RunDetailPage.test.tsx` | exposes the server-projected actions for %s | `expect(element.textContent).toContain(status === 'waiting_call' ? '等待调用结果' : status === 'ready' ? '等待执行' : '等待恢复上下文')` | `expect(element.textContent).toContain(label)` | S19/S22/S23/S24：统一等待文案/三动作 |
| W21 | `apps/web/src/RunDetailPage.test.tsx` | uses the projection for cursor and control hints despite conflicting legacy fields | `expect(element.querySelector('[aria-label="活动节点"]')?.textContent).toContain('宿主重启后尚未接续')` | `expect(element.querySelector('[aria-label="活动节点"]')?.textContent).toContain('等待接续')` | S19/S22/S23/S24：统一等待文案/三动作 |
| W22 | `apps/web/src/Timeline.test.ts` | keeps the restart message for a Run the host no longer owns | `expect(factsFor({ running: false, status: 'running', updated: '2026-09-29T10:30:00', projection: { phase: 'waiting_resume', active: false, control_requested: null, terminal: false, quiescent: false, needs_attention: true, actions: { pause: false, stop: true, resume: true }, } })).toEqual({ label: '运行状态', value: '宿主重启后等待接续', })` | `expect(factsFor({ running: false, updated: '2026-09-29T10:30:00', projection: { phase: 'executing', active: false, reason: null, updated: null, actions: { stop: true, resume: true, abandon: true }, } })).toEqual({ label: '运行状态', value: '宿主重启后等待接续', })` | S19/S20：Timeline phase/attention |
| W23 | `apps/web/src/Timeline.test.ts` | reports a resident segment by its own window, not the Run lifetime | `expect(factsFor({ running: false, status: 'stopped', activity: [] }, { resident: true, end: +new Date('2026-09-28T09:00:00') })).toEqual({ label: '这段时长', value: '暂无记录', end: '暂无记录', })` | `expect(factsFor({ running: false, activity: [], projection: { phase: 'waiting', waiting_for: 'human', active: false, reason: 'stopped', updated: null, actions: { stop: false, resume: true, abandon: true } } }, { resident: true, end: +new Date('2026-09-28T09:00:00') })).toEqual({ label: '这段时长', value: '暂无记录', end: '暂无记录', })` | S19/S20：Timeline phase/attention |
| W24 | `apps/web/src/Timeline.test.ts` | includes waiting recovery in the same attention filter used by the Timeline | `expect(entry.status).toBe('waiting_recovery')` | `expect(entry.status).toBe('waiting')` | S19/S20：Timeline phase/attention |
| W25 | `apps/web/src/parallel.test.ts` | shows both active branches, animates both selected inputs, and leaves join pending | `expect(nodes.filter(node => node.data.state === 'running').map(node => node.id)).toEqual(['branch-a', 'branch-b'])` | `expect(nodes.filter(node => node.data.state === 'executing' && node.data.live).map(node => node.id)).toEqual(['branch-a', 'branch-b'])` | S19/S20：Run 新词汇/worker 观察 |
| W26 | `apps/web/src/parallel.test.ts` | shows both active branches, animates both selected inputs, and leaves join pending | `expect(toFlowNodes(region, 'parallel', { ...state, active: {} }, observation).filter(node => node.data.state === 'running')).toEqual([])` | `expect(toFlowNodes(region, 'parallel', { ...state, active: {} }, observation).filter(node => node.data.live)).toEqual([])` | S19/S20：Run 新词汇/worker 观察 |
| W27 | `apps/web/src/parallel.test.ts` | projects native Rust branch cursors while leaving completed branches and join inactive | `expect(toFlowNodes(region, 'parallel', native, observation).filter(node => node.data.state === 'running').map(node => node.id)) .toEqual(['branch-a', 'branch-b'])` | `expect(toFlowNodes(region, 'parallel', native, observation).filter(node => node.data.state === 'executing' && node.data.live).map(node => node.id)) .toEqual(['branch-a', 'branch-b'])` | S19/S20：Run 新词汇/worker 观察 |
| W28 | `apps/web/src/parallel.test.ts` | keeps retained %s branch cursors quiet instead of rendering execution | `expect(nodes.filter(node => node.data.state === phase).map(node => node.id)).toEqual(['branch-a', 'branch-b'])` | `expect(nodes.filter(node => node.data.state === view.phase).map(node => node.id)).toEqual(['branch-a', 'branch-b'])` | S19/S20：Run 新词汇/worker 观察 |
| W29 | `apps/web/src/parallel.test.ts` | keeps retained %s branch cursors quiet instead of rendering execution | `expect(nodes.filter(node => node.data.state === 'running')).toEqual([])` | `expect(nodes.filter(node => node.data.live)).toEqual([])` | S19/S20：Run 新词汇/worker 观察 |
| W30 | `apps/web/src/parallel.test.ts` | keeps unknown Run observations quiet while retaining definition-only editor nodes | `expect(unknown.filter(node => node.data.state === 'running')).toEqual([])` | `expect(unknown.filter(node => node.data.live)).toEqual([])` | S19/S20：Run 新词汇/worker 观察 |

参数化断言的具体取值（同一表达式的每个实例）：

| 文件/测试 | 原预期 | 新预期 | 行为分组 |
| --- | --- | --- | --- |
| RunDetailPage / exposes… / waiting_call | 按钮停止、继续；调用stop/resume；等待调用结果 | 按钮停止、继续、放弃；调用stop/resume/abandon；waiting/child 显示等待子任务 | S19/S22/S23/S24：三动作/等待词汇 |
| RunDetailPage / exposes… / ready | 按钮停止、继续；调用stop/resume；等待执行 | 按钮停止、继续、放弃；调用stop/resume/abandon；executing+inactive 显示等待接续 | S19/S20/S22：三动作/Ready合并 |
| RunDetailPage / exposes… / waiting_recovery | 按钮停止、继续；调用stop/resume；等待恢复上下文 | 按钮停止、继续、放弃；调用stop/resume/abandon；waiting/effect+reason 显示等待核查效果：工具结果未记录 | S19/S22/S23/S24：三动作/等待词汇；此fixture故意独立给动作事实以检查UI不自行授权 |
| RunInspector / uses… / waiting_resume / node-summary | 等待接续 | executing+inactive 仍显示等待接续 | S20：worker观察 |
| RunInspector / uses… / waiting_resume / call-records | 本节点：等待接续 | 本节点：等待接续 | S20：worker观察 |
| RunInspector / uses… / waiting_recovery / node-summary | 等待恢复上下文 | 等待核查效果：工具结果未记录 | S19：统一reason |
| RunInspector / uses… / waiting_recovery / call-records | 本节点：等待恢复上下文 | 本节点：等待核查效果：工具结果未记录 | S19：统一reason |
| RunInspector / uses… / paused / node-summary | 已暂停 | 等待你处理：paused | S19/S21：Human统一 |
| RunInspector / uses… / paused / call-records | 本节点：已暂停 | 本节点：等待你处理：paused | S19/S21：Human统一 |
| parallel / keeps retained… / waiting_resume / branch identities | 两分支state=waiting_resume | 两分支state=executing，inactive | S20：worker观察 |
| parallel / keeps retained… / waiting_resume / live identities | state=running的节点为空 | live=true的节点为空 | S20：worker观察 |
| parallel / keeps retained… / waiting_recovery / branch identities | 两分支state=waiting_recovery | 两分支state=waiting，waiting_for=effect | S19：等待词汇 |
| parallel / keeps retained… / waiting_recovery / live identities | state=running的节点为空 | live=true的节点为空 | S20：worker观察 |
| parallel / keeps retained… / paused / branch identities | 两分支state=paused | 两分支state=waiting，waiting_for=human，reason=paused | S19/S21：Human统一 |
| parallel / keeps retained… / paused / live identities | state=running的节点为空 | live=true的节点为空 | S20：worker观察 |
| parallel / keeps retained… / stopped / branch identities | 两分支state=stopped | 两分支state=waiting，waiting_for=human，reason=stopped | S19/S21：Human统一 |
| parallel / keeps retained… / stopped / live identities | state=running的节点为空 | live=true的节点为空 | S20：worker观察 |

fixture 的间接断言变化与未覆盖边界：

- `Timeline.test.ts / includes waiting recovery…`：原通过手工切换 needs_attention=false 检查attention；新将 waiting/effect 切换为 settled/done 后仍断言false，禁止隐藏的attention旗标。原true/false断言均保留。
- `RunDetailPage.test.ts / shows recovery guidance…`：原恢复专用文案来自waiting_recovery分支；现只来自projection.reason。通过state.error带旧文案但projection无该reason验证不泄露旧字段；原正反文案断言保留。
- `ChannelsPage.test.ts / does not infer resumability…`：旧stopped但quiescent=false的fixture现在映射waiting/human+inactive，按新语义允许退役（disabled=true→false）；Run controls仍全部拒绝。属于Human静止统一的可见行为变化。
- `e2e/recovery.spec.ts / unknown tool result resumes…`：旧waiting_recovery fixture允许resume；为保留原resume成功操作验收，新fixture显式waiting/human并携同一恢复说明。它只验证人等待后的接续，不证明effect等待可以resume；effect按新动作规则不允许resume，停止条件后未新增独立effect组件测试。恢复副作用、产物、实际provider未由这个mock验证。
- `RunDetailPage / exposes…` 的三个fixture都故意提供stop/resume/abandon=true，验证UI只读Host动作；不代表这些phase真实Host都会授权这三项。
- 其余typed Run/CallRecord、mocked e2e的projection只将旧Run词汇与flags迁为新字段；Session Turn、native branch status、listener、节点完成/中断事实不迁移。原非状态断言全部保留。
- `Timeline` 的finished类选择器迁为settled（原宽度、时间、几何断言未变）；AST清单只统计expect表达式，selector绑定在变量赋值中的变化需按此补记。

验证：npm test 全量33文件285通过；npm run build通过。mocked e2e首轮10例9过/1文案selector不唯一，收窄badge selector后该例独立复跑通过。日志入口为/tmp/r2-web-test.log、/tmp/r2-web-build.log、/tmp/r2-web-e2e.log、/tmp/r2-web-e2e-recovery.log。真实provider未运行。

定版验证的确切命令（cwd=`apps/web`，最终 src 改动后再次执行）：

- `npm test`：exit 0；33 files / 285 tests passed；`/tmp/r2-web-test.log`。
- `npm run build`：exit 0；Vite built in 9.00s；`/tmp/r2-web-build.log`；TypeScript 包含 e2e 编译。
- 定向浏览器命令（repo root）：`ANCHOR_BROWSER_BINARY=/opt/google/chrome/google-chrome ANCHOR_WEB_TEST_PORT=5192 npm --prefix apps/web run test:e2e -- e2e/recovery.spec.ts e2e/graph-calls.spec.ts e2e/parallel.spec.ts e2e/timeline.spec.ts --workers=2`，exit 1，9 passed / 1 failed（reason 展示重复导致原 selector 不唯一）。
- 对失败例独立复跑（repo root）：`ANCHOR_BROWSER_BINARY=/opt/google/chrome/google-chrome ANCHOR_WEB_TEST_PORT=5192 npm --prefix apps/web run test:e2e -- e2e/recovery.spec.ts --grep 'unknown tool result' --workers=1`，exit 0，1 passed。不能写成同一次10例全通过。
- 更早一次无效定向命令：`npm test -- RunDetailPage RunInspector Timeline parallel persistentAssistant ChannelsPage` 使 vitest 的 OR 路径过滤额外接纳了3个 Playwright suites；3 suites failed / 原285 tests passed。原始日志 `/tmp/r2-web-focused.log`；随后使用正确的 `npm test` 全量通过。

清单计数口径：30条直接变更的断言表达式（W1–W30）；17条参数化实例解释（与W条目存在展开关系，不应直接相加）；6条fixture/边界说明及1条selector变量补记。组件测试用例总数没有减少。原产物、恢复成功、时间/几何、请求身份断言保留；状态派生退役按钮的disabled=true→false明确登记为human静止语义变化。
