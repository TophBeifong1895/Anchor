# 设计卡 002：删掉批准的第二本账

> **状态：已获用户决定（2026-10-10）——A "删除干净"。** 这不是加功能，是**删机制**。
> 判据（用户提出的收敛准则）：切片必须**减少**"跨事实必须一致"的点；增加状态/状态机/对账的切片是在付利息。

## 1. 被删的东西（第二本账）

宿主在平台会话之外**又**维护了一套批准状态机，与平台已有的 `Question` 记录描述同一件事：

- `state/environment-plans/<身份>-<名字>-<会话>-<回合>.json`（`EnvironmentPlanStatus`：Requested/Published/Refused/Aborted/Cancelled）
- `state/environment-provision-plans/…`（`EnvironmentProvisionStatus`：同上）
- 围绕它们长出来的：`write_*_plan`、`abort_*`、`cancel_*`、`settle_abandoned_*`（启动对账）、`prune_environment_records` + `Retention` + `RecordList` + `pruned.count`、`unreadable/truncated` 投影字段、两个只读投影 `GET /environment-publications`、`GET /environment-provisions`。

全仓引用这两个状态类型的位置 **99 处**，其中 **22 处**专门判断 `Requested`——每一处都是"两本账必须一致"的利息。

## 2. 权威所有者（删完之后只剩两处）

- **"人被问过什么、答了什么"** → 平台会话的 `Question`（`anchor-platform-session`，已落盘：`message`、`requested_schema`、`status: pending|answered|interrupted`、`answer`）。宿主只读。
- **"发布了什么内容、名字指向哪个编号"** → `Library`（已发布树不可变 + `names.json` CAS）。宿主已经在用（`environment_list`/`environment_show`）。

宿主**自己拥有**的事实保留：`state/environment-requirements/<名字>.json`（"管家申请了什么环境"——没有别人拥有它）。

## 3. 删掉之后，状态怎么得到

不再持久化"当前状态"，改为**当场判断**：

- 在**本次调用内**，是否执行由表单返回的答案决定（绑定用的候选身份/名字/观测版本仍在内存里的 precondition 中）。
- 跨重启**不重放**：这正是既有产品语义（"待批计划不会自动重放"），删账后它变成结构性质而不是一条需要维护的规则。
- "宿主发布过什么"由 Library 回答；"有没有正在等回答的表单"由会话自己的提问接口回答（GUI 已有 `GET /sessions/{s}/turns/{t}/questions` 路由）。

**后果（要在台账如实写）**：两个跨会话的只读投影被删。它们展示的事实仍可见（Library 列表 + 会话提问路由），但"把所有批准历史汇到一处看"这个视图没有了。要把它找回来，正确做法是**让平台给 question 打类型标签**（改 `anchor-platform-session`），而不是在宿主里再记一本账。本卡不做。

## 4. 不变量（删账后必须仍然成立）

- 批准**不是**工具参数；未确认不执行。
- 发布前复核内容身份（候选身份与批准时一致），不一致拒绝。
- 名字切换仍是 CAS（观测版本不符则拒绝）。
- 磁盘上已有的旧计划文件**不删用户数据**：宿主不再读也不再写它们，文件留着不动。

## 5. 验收（删完必须成立）

1. 发布成功路径仍通过：内容进 Library、名字指向新编号（`environment_publish` 既有用例）。
2. 未确认（人答否）→**不发布**、名字不动。
3. 内容在批准后被改动 → 拒绝、不发布（既有用例）。
4. `environment_list`/`environment_show` 仍工作（Library 派生）。
5. **不再有任何计划文件被写入**：跑完发布流程后，`state/` 下不存在 `environment-plans/`、`environment-provision-plans/` 目录（新用例断言）。
6. 全量测试通过；**测试数量应下降**（删掉的用例多于新增的），台账写下删了多少、留下多少。
7. 不留死代码：`EnvironmentPlanStatus`/`EnvironmentProvisionStatus` 及其 99 处引用、`prune_*`/`Retention`/`pruned.count`、`api.rs` 的两个投影 handler 与映射全部删除，`clippy -D warnings` 仍然干净（unused 会逼出遗漏）。

## 6. 明确不做

- 不改 `anchor-platform-session`（不给 question 加类型标签）。
- 不为"批准历史"新建任何存储、索引或反向表。
- 不动内容链（解析/抓取/构建/物化/发布/验证）与 Run 冻结语义。
