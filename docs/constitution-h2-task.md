# H2 任务书：节点失败结算 + 调度器

当前结论：**H2.1 WIP，触发 §5 停止条件；H2.2 / H2.3 未开始**。并行 seed 495 的多失败错误文本不一致，见[阻塞证据与交回](../experiments/constitution-host-preflight/h2/verification.md)。

编制：2026-10-17。上位计划：[constitution-host-integration-plan.md](constitution-host-integration-plan.md) 的 H2。

本文拆成三刀：H2.1、H2.2 要实现；H2.3 **只做调查并交回**，不接线。每刀结束必须编译通过、测试通过、更新工作记录并单独 commit。

## 0. 开工前

1. 按顺序读：`AGENTS.md`；上位计划第 0、2 节（约束 C1–C6、§2.1 调度器、§2.2 执行者）；`docs/constitution-refactor-log.md`（S1 起的语义变更、H1 结论）。
2. **worktree 当前编译不通过**，原因是有一处未提交的半成品。`rust/anchor-runtime/src/graph/validator.rs` 已加入 `SettleSubmission` 结构，`judge` 里也已调用 `judge_settle`，但这个函数还不存在。H2.1 直接在此基础上完成，按 §1 的形状修改。不要用 `git checkout`、`reset` 或 `stash` 丢弃改动。
3. 记录基线：`cd rust && cargo build -p anchor-sandbox-bwrap --bins`，然后 `cargo test -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host`。H2.1 完成前编译不过，基线就用 `6ee2b0e` 交接时工作记录里的数字，并注明。

不要重做的东西：内核六原语、Graph 校验器的串行/并行/调用裁决、三组差分测试、H1 的旧格式拒绝。

## 1. H2.1 校验器：节点失败结算

### 语义（以旧 Runner 为准）

旧 `NodeExecutionOutcome::Failed { reason }` 对应 `runner.rs` 的 `fail_known_node` 和 `parallel.rs` 约 271 行的分支。这是宿主已确认的节点失败，不是可重试的错误。旧语义如下，新实现必须逐条保持：

- 该调用计为已开始：`invocations[node]`、`passes[node]` 照常累加。
- 不记录节点结果，不决定任何出边（`decided` 不变），下游节点不会开启。
- Run 终结为失败，错误文本为 `"{node} failed: {reason}"`。
- 在并行区内：该分支 `status = Failed`、`error = reason`，`record.parallel` 保留；已完成兄弟分支的结果保留；join 永不开启。之后兄弟分支迟到的提交由内核以 `TaskSettled` 拒绝（与 S6 一致）。

### 形状（已决定）

- 提交：`Action::Settle`，`slot` 为失败调用的槽位，内容为 `SettleSubmission { reason: String }`，`deny_unknown_fields`。**删除半成品里的 `node_id`、`invocation` 字段**：槽位已经指明调用，再写一遍只会制造不一致的可能。
- 只接受 `PrincipalKind::Code`。其他身份返回 `Refused(Unauthorized)`，因为失败由宿主观察，不由模型自述（C5）。
- 只接受 `DispatchKind::Work`。Fanout 和 Join 是确定性宿主代码，不会失败；Call 已有 `CallSubmission::Failed`。这三种返回 `Refused(SlotClosed)`。
- 结算相位：`Phase::Settled { result: Outcome::Failed { reason: Reason::Unclassified, detail } }`，`detail` 为上面的错误文本。原因码用 `Unclassified`，与 `conclude` 中分支轮次上限失败的写法一致。**不得为此在内核新增 `Reason` 变体**：那是内核改动，触发停止条件。
- 状态更新复用 `count_dispatch`，不调用 `record_result`，然后用 `accept(&record, verification_of(s), Some(phase))` 返回。

### 测试

1. 单测：新建 `validator/settle_tests.rs`，写法参照 `validator/interruption_tests.rs`。至少覆盖：
   - 串行：计数增加，`results` 与 `decided` 不变，下游节点没有计数，相位为 Failed，detail 包含节点名和原因。
   - Agent 身份被拒（Unauthorized）。
   - 已关闭的槽位被拒（SlotClosed）。
   - 内容中有多余字段时返回 `Returned(ContentInvalid)`。
   - Fanout、Join、Call 槽位被拒。
   - 经 `Kernel` 结算后，再提交到任何槽位都被拒（TaskSettled）。
   - 并行：一个分支失败后，兄弟分支已完成的结果保留，该分支为 Failed，join 不开启，兄弟分支迟到的提交返回 TaskSettled。
2. 差分：给 `validator/differential.rs` 的串行组和并行组加失败注入。用 `hash(node, invocation, seed)` 决定，概率约 3%。旧侧在 `Scripted::execute` 返回 `NodeExecutionOutcome::Failed`，新侧用 Code 提交 Settle。Tally 增加 `node_failed` 字段，串行组和并行组各自要求 `node_failed > 100`。**已有阈值一律不得下调。**并行组里节点失败与轮次上限失败目前走同一个 `old.status == Failed` 分支，那里断言了 `ceased` 非空，需要按失败原因分开处理。比较的事实包括错误文本（旧 `record.error` 与新 `detail`）。
3. 在工作记录中新增一个语义条目（编号沿用最后一个 S 之后）。如果实现中发现与旧语义有任何差异，逐条写出。

提交：`feat(validator): settle a Run on a known node failure (H2.1)`

## 2. H2.2 调度器（anchor-runtime，Host 暂不接线）

### 位置与形状（已决定）

- 新文件 `rust/anchor-runtime/src/graph/scheduler.rs`。调度器放在这里而不是 runner-host，是因为 runner-host 和 graph-host 都要用它，而 anchor-runtime 不是内核，不违反 C1。
- 调度器对存储泛型：`Kernel<S: Store>`。测试用 `MemoryStore`，重启测试用 `FileStore`。
- **执行端口直接复用现有 `NodeExecutionPort` 和 `ArtifactPort`**，不新造执行者 trait。这样 H2.3 接线时，现有 node_host 实现可以原样插入；对照测试里新旧两侧也能用同一个 fixture 端口。
- `NodeExecutionRequest` 由现有的纯函数构造（`runner.rs` 中 `execution_request` 的逻辑）。需要的话把它移到两侧共享的位置，**不许复制一份**。fanout/join 的 manifest 构造（`runner.rs` 约 540–580 行）同样抽成共享函数，旧 Runner 也改为调用它。
- 不新增环境变量开关，不改 `execution.rs`。

### 语义

循环见上位计划 §2.1：`load` → 若相位不是 Executing 则退出 → `GraphValidator::frontier` 取得开放的派发 → 每个派发若尚无任务在跑就启动 → 拿到结果后提交 → 重复。具体要求：

1. **并发**：同一 Run 的多个开放槽位并发执行，不设波次屏障。所有提交都只经过 `kernel.submit`。
2. **版本**：提交时带派发时看到的版本。收到 `StaleVersion` 后重读版本、原样重提，次数不限。收到 `TaskSettled` 不重试，计入迟到。
3. **身份与成本**（C5）：Work 结果用 `PrincipalKind::Agent` 提交，fanout、join 和失败结算用 `PrincipalKind::Code`。成本取 `NodeCompletion.model_requests`，由调度器在 `submit` 的 `cost` 参数中盖章，不从提交内容读取。
4. **产物**：`ArtifactPort::freeze` 在提交前调用，`commit` 放进 `NodeSubmission`，并附 `Evidence::Resource`。写法照差分测试新侧。
5. **先核查再执行**（重启语义）：派发 Work 前先调用 `completion_fact(key)`，映射与 `runner.rs` 约 286–311 行一致：
   - `Completed`：直接提交，不执行。
   - `Failed`：提交 Settle。
   - `Uncertain`：提交 Settle，detail 为 `"uncertain node result for {node}: {reason}"`。这与旧行为一致；是否改为"等待人"留给 H2.3 之后再定，在工作记录里登记为待决。
   - `NotStarted` / `Resumable`：执行。
   - `Yielded`：返回具名的不支持错误（H4 处理）。
6. **执行结果映射**：
   - `Completed`：提交。
   - `Failed`：提交 Settle（H2.1）。
   - `Cancelled` / `Suspended` / `Interrupted`：不提交，等待其他在跑的任务收束后退出。
   - `Yielded`、`WaitingRecovery`、`BudgetExhausted`、`Aborted`、`DispatchKind::Call`：本刀返回**具名**的 `GraphError::Unsupported`，错误信息写明由哪一刀处理，不得静默吞掉。
7. **停止**（C4）：调度器不读 `stop_requested` 或 `pause_requested`。外部提交 stop 被接受后，相位不再是 Executing，调度器在下一轮自然退出。传给执行端口的 `cancellation` 由调用方持有；本刀只需保证相位离开 Executing 时调度器退出，并等待在跑任务结束。

### 测试

1. **Host 级对照**：对差分测试的串行与并行随机 Graph 生成器各取至少 2,000 个 seed，同一个 fixture `NodeExecutionPort` 分别交给旧 `GraphRunner` 和新调度器执行。fixture 要含 H2.1 的失败注入。
   - 串行：派发序列（key 加输入 commit）必须完全相同。
   - 并行：调度器没有波次屏障，所以比较派发多重集。
   - 两者都要比较最终 Graph 事实（复用 `facts`、`remove_phantoms` 的处理，以及 S6 迟到规则）。
2. **重启**：用 `FileStore`，在第 N 次执行时让 fixture 返回错误或 panic，使调度器中断。然后用同一目录新建 Kernel 和调度器，断言两点：已提交的调用不再执行；被中断的调用以同一 invocation 号经 `completion_fact` 恢复。fixture 对它分别返回 Completed 和 NotStarted 各测一次。
3. **并发与版本**：至少一个测试断言并行区确实并发（两个 fixture 节点用屏障互等，串行执行会超时），并断言 `StaleVersion` 重提路径被走到。
4. **停止**：执行中由另一身份提交 `{"control":"stop"}`，调度器退出，在跑任务的结果按内核规则被拒（`TaskWaiting`）并留在历史里。

提交：`feat(runtime): kernel scheduler over existing node ports (H2.2)`

## 3. H2.3 接线前调查（只读，不改代码）

接线的难点不在调度器本身，而在"中间态单一权威"：Run 一旦由内核 `FileStore` 负责，Host 里 H1 基线所列的 16 处 `store.save`、36 处字段修改，以及所有按 `RunStatus` 读取的地方，都会变成第二个权威。顺序没想清楚之前不要接线。

新建 `experiments/constitution-host-preflight/h2/wiring-survey.md`，回答下面的问题。每条结论都附 `文件:行`：

1. `execution.rs` 和 `graph_call.rs` 中构造 `GraphRunner` 的每一处：调用方是谁，传入的 `RunStore`、`RunControl`、端口分别是什么。
2. H1 基线的 16 + 36 处直接写入：逐条标注在 Run 迁到内核后，它应当变成哪一种：
   - 内核生命周期提交；
   - Code 提交；
   - Session 层事实（不属于 Run）；
   - 可以直接删除。
3. 读取 Run 状态的地方（`RunStatus::`、`record.status`、`record.error`、`record.recovery`）：按读者分组，例如 API 投影、Web、助手、恢复、删除。对每组说明能否从 `Resource.phase()` 加 `GraphRunRecord` 投影得出；不能的，说明缺什么。
4. Op.call 的子 Run：父 Run 进入 `Waiting::Child` 之后，谁观察子 Run 结算、谁提交父 Run 的 `resume` 以及 `CallSubmission::Completed/Failed`。现有 `GraphCallPort` 能否直接充当这个"宿主代码"。
5. `WaitingRecovery`、`BudgetExhausted`、`Aborted` 在现有 Host 中的产生点和人工决定入口（`record_recovery_decision`）。说明它们映射到内核"等待人加 resume/abandon"需要哪些提交内容，以及现有内核 `task_control` 是否已支持（读 `anchor-kernel` 源码确认，不要猜）。
6. 给出接线顺序建议。要求每一步结束时测试全绿，且任一时刻每个 Run 只有一个权威。标出你认为需要内核或校验器改动的点。

提交：`docs(preflight): H2 wiring survey (H2.3)`。交回后由审查方编写接线任务。

## 4. 硬约束复述（每刀验收都查）

- C1：`git diff 6ee2b0e -- rust/anchor-kernel` 必须为空。
- C3：`validator.rs` 及其子模块不得出现 I/O、async、端口调用。
- C4：`grep -rn "stop_requested\|pause_requested" rust/anchor-runtime/src/graph/scheduler.rs` 必须零处。
- C5：调度器中的成本只来自 `submit` 的参数，`Agent` 身份不得提交 Settle、fanout 或 join。
- 不新增 `#[ignore]`，不删除、不弱化已有断言或 Tally 阈值。确有必要修改的，在工作记录中逐条写明理由。

## 5. 停止条件（遇到即停，写明原因交回）

- 需要修改 `anchor-kernel` 的任何代码，包括新增 `Reason` 变体。
- 需要修改校验器中 H2.1 之外的裁决逻辑。
- 旧 Runner 的行为与本文 §1 描述的语义不一致。
- 对照测试出现无法用已登记的语义变更（S1 起）解释的差异。
- 重启恢复需要在 Run 状态里新增"已派发"标记才能做到。
- 对照测试或重启测试只能靠削弱断言才能通过。

## 6. 验收命令（交回前自己跑一遍，原样贴输出摘要）

```bash
cd /home/mansteinl/code/anchor-constitution-core/rust
cargo build -p anchor-sandbox-bwrap --bins
cargo fmt --all -- --check
cargo clippy -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host --all-targets -- -D warnings
cargo test -p anchor-kernel
cargo test -p anchor-runtime -- --nocapture differential   # 贴出每组 Tally
cargo test -p anchor-runtime
cargo test -p anchor-runner-host -p anchor-graph-host
git diff 6ee2b0e --stat -- rust/anchor-kernel              # 期望为空
```

## 7. 交回模板（每刀一份，填完贴给审查方）

```
刀：H2.x
commit：<hash> <标题>
改动文件：<列表>
测试：kernel <通过/失败/忽略>；runtime <…>；runner-host <…>；graph-host <…>
  与基线差异：<逐项；无则写"无">
差分 Tally：serial <…>；parallel <…>；calls <…>（node_failed 数值必填）
对照测试（H2.2）：seed 数 <…>，差异 <…>
clippy / fmt：<通过/失败>
结构检查（§4）：C1 <…> C3 <…> C4 <…> C5 <…>
新增语义条目：<编号与一句话>
未执行的项：<逐项写"未执行"及原因；无则写"无">
停止条件触发：<无 / 具体是哪条及现场>
需要审查方决定的问题：<…>
```
