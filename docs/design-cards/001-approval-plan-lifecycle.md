# 设计卡 001：批准请求的终态与记录无界增长

> **状态：已批准（2026-10-10）——D1B / D2B / D3A / D4A（见 §8、§9）。** 本文是**提案**，不是已接受的决定；批准后其中被接受的决定并入
> `docs/dependency-environment-draft.md`（ADR）与 `docs/pilot-development-plan.md`（台账），本文保留为提案记录。
> **批准之前不写任何实现代码。**
>
> 本卡遵循的流程：先出设计卡（含机制可行性证据与验收清单）→ 用户批准 → 按卡施工 → 任何偏离设计卡的
> 设计变更停下更新本卡再问 → 完成后做**验证型**审查（对着第 6 节的验收清单逐条要证据）。

## 1. 问题（读码 + 两路独立审查，均已核实）

计划记录在**弹出原生表单之前**就落盘为 `Requested`（`application/environments/plans.rs:195-199`；
`environment_provision` 同形）。但有三处早退**不写终态**：

| 路径 | 位置 | 现状 |
| --- | --- | --- |
| 内容已发布，直接回"已是最新" | `goose_acp/pilot_interaction.rs:438-443` | `return Ok(...)`，记录停在 `Requested` |
| 表单传输失败 / Pilot 取消（publish 与 provision 同形） | `pilot_interaction.rs:469-476`、`:702-709` | `return Err(...)`，记录停在 `Requested` |
| 同上，`plugin_install` | `pilot_interaction.rs`（install 分支） | 无计划记录，仅消息层面（本卡不改） |

保留策略把 `Requested` 当作"人还可能回答"，**永不删除**（`application/environments/mod.rs:155, 209-234`），
且 `prune` 只在写终态时触发（`plans.rs:108`）。两个后果：

- **A（诚实性）**：只读投影把"没人被问过"报成 `"status":"requested"`（`api.rs:612-616, 666-671`），而该投影的
  用途原文正是区分"已发布"与"未被回答的请求"。
- **B（无界增长）**：计划键含 `session`+`turn`（`plans.rs:79-84`），模型可以反复对一个候选调
  `environment_publish`/`environment_provision` 并让回合结束 → 每次留一条**永不回收**的记录。

## 2. 目标

1. 任何计划记录都不得停留在"已经没有人能再回答"的 `Requested`。
2. 记录总量有界，且**删除量可观测**（不得静默丢弃已落盘事实）。
3. 终态**不发明人的决定**：人没回答 ≠ 人拒绝 ≠ 批准后没做成。

## 3. 归属与事实所有者

- **计划记录**归宿主 state（`application/environments`），已有形状不变。
- **"人还能不能回答"**是**平台会话**的事实：`anchor-platform-session` 在 turn 停止时把待答问题批量标为
  `interrupted`（`turns.rs:485` → `questions.rs:154-167`）。宿主只**读**这个事实，**不改写**它。
- 依赖方向不变：`anchor-platform-session` 不引入对宿主计划记录的任何依赖。

## 4. 不变量

- **I1** 记录不得停在无人能答的 `Requested`。
- **I2** 三种终态不得混用：`Refused`=人答"否"；`Aborted`=人批准了但事没做成；`Cancelled`=人从未回答。
- **I3** 保留策略不得删除：`Requested`、读不出的记录、**名字当前指向**的那条成功记录。
- **I4** 每一处删除都计数，并在只读投影里可见。
- **I5** 投影不得把"没被问过"读成"没人回答"，也不得在覆盖度不完整时自称完整。
- **I6** 计划记录的写入**不要求** turn 活跃（见 §5），因此取消路径**可以**写终态。

## 5. 机制可行性（已核实的证据 / 未证实的假设）

已核实（读码）：

- `TurnStatus { Running, Completed, Failed, Stopped, Interrupted }`（`anchor-platform-session/src/lib.rs:234-240`）。
- 平台在 turn 停止时批量把待答问题标为 `interrupted`（`turns.rs:485` → `questions.rs:154-167`）：
  **"人已无法回答"已经是落盘事实**。
- 启动已有对账钩子 `recover_pilot_turns`（`api/turns.rs:20`，由 `api.rs:287` 调用）。
- `write_environment_plan` 只做文件 IO + prune，**没有** turn 前置条件（`plans.rs:90-119`）；
  `abort_environment_plan` 已在"批准后失败"路径使用过（`pilot_interaction.rs:511-517`）。

实施前复核（2026-10-10，批准后、写代码前完成）：

- **已确认**：turn 终结统一经由 `finish(...)`（`anchor-platform-session/src/turns.rs:478-492`），而它**总是**
  调用 `crate::questions::interrupt_pending`；`interrupt_running`（同文件 `:214`）也走 `finish`。因此
  `stop_pilot`（`anchor-runner-host/src/api/turns.rs:253` → `pilots.stop`）与 channel 中断最终都会把待答问题
  标成 `interrupted`——"没人能再回答"在任何停止路径上都是落盘事实，不需要新机制。
- **已确认**：`environment_publish` 的取消分支与传输失败分支都在 `active(...)` 之前 `return Err`
  （`goose_acp/pilot_interaction.rs:469-476`、`:702-709`），因此在这些路径上写终态**不受 turn 活跃守卫阻挡**
  （不变量 I6 成立）。
- **已确认**：`write_json_durable` 是唯一临时名 + fsync + rename（`application/environments/mod.rs:58-92`），
  同一 key 的并发写是"最后写入者胜"，不会撕裂。

## 6. 验收测试清单（每条都必须能红）

| 编号 | 用例 | 断言 |
| --- | --- | --- |
| T1 | 弹表单后取消 | 计划终态是 D2 选定的终态；投影不再显示 `requested` |
| T2 | 对同一候选第二次调用且内容已发布 | 计划终态为 `Published{observed_id, observed_name_version}`（D3 选 A） |
| T3 | 表单传输失败 | 同 T1，且**不**留下 `requested` |
| T4 | 同一名字被反复取消 N 次（N > 上限） | 目录记录数 ≤ 上限，且投影报告 `pruned ≥ 1` |
| T5 | 某名字已发布后累积超过上限的终态记录 | 名字当前指向的那条 `Published` 记录**仍在**（I3） |
| T6 | 目录里放一条坏 JSON | 它仍在、且计入 `unreadable`，不被删除（I3） |
| T7 | 启动时存在所属 turn 已非 Running 的 `requested` 记录 | 被收尾为终态（仅当 D4 选 A） |
| T8 | 用旧记录的 JSON 反序列化新代码 | 新增字段有 `#[serde(default)]`，旧记录仍可读 |

## 7. 明确不做

- 不改 Run 接纳/冻结语义，不改名字表 CAS，不改 `publish_expecting` 的身份校验。
- 不引入通用 GC、后台清理线程或定时任务。
- 不改 `anchor-platform-session`（只读它的事实）。
- 不为 `plugin_install` 新增计划记录（它没有需要人回答的持久计划；本卡只保证取消时消息不撒谎）。
- 不顺手做 A188 里登记的其他项（删除保护 fail-open、`impact_scan` 线程、发布持图锁……）。

## 8. 需要你拍的决定

- **D1｜已落盘的记录能不能被物理删除？**
  - A 全保留：不删任何记录，只截断投影（已有 200 上限 + `truncated`）。最保守，目录无界增长。
  - **B（建议）** 只删"可证明冗余的终态记录"（同一名字+同一身份已有更新的终态），永不删 `Requested`/读不出/名字当前指向的那条，并把删除数落成 `pruned` 计数。
  - C 维持现状（每名字最新 20 条终态），补计数并保护"名字当前指向"的那条。
- **D2｜"人从未回答"（取消）用什么终态？**
  - A 复用 `Aborted{reason}`：改动最小，但把"没做成"和"没人回答"混在一类。
  - **B（建议）** 新增 `Cancelled{reason}`：与 `Refused`/`Aborted` 分清，投影多一个状态。
  - C 不写终态，改为读取时派生 `expired`：不改落盘事实，但待批记录仍永不被回收（与 I1 冲突）。
- **D3｜`already_published` 早退要不要留终态记录？**
  - **A（建议）** 写 `Published{observed_id, observed_name_version}`：它确实已发布，事实为真。
  - B 改成"只在会弹表单时才写记录"：记录更少，但写入位置要重排，风险更高。
- **D4｜过期的 `requested` 是否允许自动收尾（改写落盘事实）？**
  - **A（建议）** 启动对账时收尾（`recover_pilot_turns` 已有钩子），理由：`Requested` 永不回收会与 I1 冲突。
  - B 只在读取时派生 `expired`，不改写落盘事实：更保守，但与 I1 的冲突以"投影层解决"。

## 9. 批准

**2026-10-10：批准，四项决定按建议执行。**

| 决定 | 结果 |
| --- | --- |
| D1 记录能否物理删除 | **B**：只删"可证明冗余的终态记录"，永不删 `Requested`/读不出/名字当前指向那条；删除数落成 `pruned` 报进投影 |
| D2 取消的终态 | **B**：新增 `Cancelled{reason}`，与 `Refused`/`Aborted` 分清 |
| D3 `already_published` 早退 | **A**：写 `Published{observed_id, observed_name_version}` |
| D4 过期 `requested` 收尾 | **A**：启动对账（`recover_pilot_turns` 钩子）把所属 turn 已非 `Running` 的记录收尾为终态 |

执行纪律：§7"明确不做"之外的任何设计偏离，停下更新本卡并请示，不在代码里悄悄改语义。
