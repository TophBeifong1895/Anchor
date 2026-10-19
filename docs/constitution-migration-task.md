# 全量迁移任务书：Host 全部 Run 走内核，删除旧 Runner

编制：2026-10-19。决定：用户选择全量迁移（见工作记录"R2 审查结论"）。目标是**统一定律、崩溃与计费正确、删除旧 Runner**，不是让 Host 变小。起点：`d915314`（R2 停止现场）。

**当前状态（2026-10-19）**：M1 已验收（`2d70ca2`，审查方重跑 workspace 121 个 target 1922 / 0 / 115、Web 290 通过）。**M1.5 已验收（§10.10）**：先以 `c6454c3` 合并 `2b3b7a2`，再按 §10.8 仅同步 F0 `e3a5778`，未合入 F1。workspace 2023 / 0 / 122，Web 311 通过且构建通过，Goose 117 / 2 / 0，相对 §10.9 基线新增失败 0；规则清单与原样 target 结果见 [M1.5 验证](../experiments/constitution-host-preflight/m15/verification.md)。M2 已按 §10.10 开工；并行模块作用域扩展已获用户批准并实现，校验器与 5,000 模块随机样本验证通过，Host 多槽位调度已通过定向与随机对照；FileStore 删除操作已按用户批准实现并通过 Kernel 测试，继续 Host 删除与其余能力，见 [删除边界](../experiments/constitution-host-preflight/m2/deletion-boundary.md)；验证见 [M2 验证与当前边界](../experiments/constitution-host-preflight/m2/verification.md)。M1.5 要求如下：

1. 按 §10.2 合并 `master`，冲突逐文件登记；合并后 workspace 与 Web 全绿。
2. 按 §10.3 用 master 的 Goose 锁定版本跑全部 Goose 忽略测试，原样贴出结果。**注意**：master 目前的 Goose 基线本身就是坏的（extensions/add 返回 -32603，由 master 的 G0 任务处理，见 `/home/mansteinl/Anchor/docs/goose-baseline-task.md`）。G0 修复之前，门槛改为"相对 master 基线没有新增失败"。master 侧基线**优先使用 G0 交回的失败名单**；如果 G0 还没交回，就用临时干净 worktree 在本次合并的那个 master 提交上跑，例如 `git worktree add /tmp/master-base <提交>`，用完删除。**不得**在 `/home/mansteinl/Anchor` 上跑基线，因为那里有其他 agent 正在修改。Goose 套件不要和 G0 同时跑：G0 正在跑时，先完成合并和规则表，Goose 对照放到最后。M1 时分支锁定的是旧摘要，本机没有对应二进制，所以不能执行；合并后不再适用这个理由。
3. 补交 §10.1 的规则清单表：分别在 `1d1ba54` 与合并后的代码上统计。M1 交回时缺了这一项。
4. 从 M1.5 起执行 §10.6：原始日志不再入库。

S19–S24、S26 已由用户批准（2026-10-19）。M3 的前置条件见 §10.7：master 完成 F2，并已合入本分支。

本文取代上位计划 H3–H6 的阶段划分；上位计划的约束 C1–C6、§2.1 调度器、§2.2 执行者语义仍然有效。

当前进度（2026-10-19）：M1 已验收，S19–S24、S26 已批准，S25 作废。M1.5 已完成 `2b3b7a2` 与 F0 的同步、规则清点与相对 G0 无新增失败的验证，按 §0 交回；证据见 [M1.5 验证](../experiments/constitution-host-preflight/m15/verification.md)。M2 进行中，并行模块作用域校验器切片已完成本地确定性验证，Host 并行调度切片也已完成；Op.call 等其余能力未完成，FileStore 删除操作已获批并验证，Host 删除接线进行中；M3–M5 尚未开始。

## 0. 工作方式（先读）

- 迁移分 5 个里程碑（M1–M5）。**每个里程碑结束时交回一次**，中途只在遇到 §8 的"必须停下"时交回。其他问题自行判断、登记后继续，审查方在里程碑结束时统一处理。
- 每个里程碑内部按需要拆多个 commit，**每个 commit 编译通过、全部测试通过**。不允许留红的中间提交，WIP 除外：WIP 只在触发"必须停下"时使用。
- 必读：`AGENTS.md`；`docs/anchor-constitution.md`；上位计划第 0、2 节；工作记录中的语义表 S1–S26，以及 P、R、R2 三节；`experiments/constitution-host-preflight/p/measurement.md` 的"未决"。
- 不要用 `git checkout` / `reset` / `stash` 丢弃改动；只 stage 自己改的路径。

**测试修改规则**（全文适用）：

- **状态词汇类断言**（旧 status 字符串、needs_attention、quiescent、旧 phase 名）：可以改为新词汇，逐条登记。
- **旧 Runner 内部机制断言**（cursor、RunStore 保存次数、`stop_requested` 轮询、旧 record 字段）：随旧代码删除时，每个被删测试必须在登记表中给出"新路径的等价测试名"，或"该行为已不存在"及理由。
- **业务行为断言**（产物、成本、路由、授权拒绝、不重放、Session 投递）：**不得放宽**。行为确实改变的，登记为 S 候选；不能用改测试的方式掩盖。

## 1. M1：完成 R2（对外状态词汇）

从 `d915314` 继续。

**审查决定，修正 R2 任务书的错误**：pause 不是 stop 的别名。

- pause 是控制动作，不是状态：Executing 且有 worker 时可用。宿主记录这个意图，调度器在**当前节点结果提交之后、下一次派发之前**提交人的 `stop`，reason 为 `paused`。
- 结果状态是 `waiting / human / reason=paused`。
- `actions` 增加 `pause`。对外状态仍然只有三种相位，词汇不变。
- 旧 Run 的 pause 维持旧 Runner 原行为。
- S25 作废；按这个定义，R2 停下来的那些 pause 测试应能原样通过。

完成 R2 任务书中未完成的项：

- 旧 Run 与内核 Run 在三种处境下的同一 HTTP 对照；
- Pilot 的 pause / stop 独立测试；
- effect 等待的 Web 控件测试；
- 六 crate 全量回归。

其余按 R2 任务书执行。S19–S24、S26 保持"待批"，审查方在 M1 交回时统一处理。

**M1 完成标准**：Rust 六 crate 与 Web 全绿；对外 API 和 Web 只出现新词汇；断言修改登记完整。

## 2. M2：内核路径能力补齐

目标：P 探针中所有返回 `Unsupported` 的能力，都在内核路径上实现。完成后，任何 Graph 都能走内核路径。旧路径仍保留，作为默认入口，同时作为对照。

逐项实现，每项单独 commit。每一项把旧路径中覆盖该能力的测试复制一份，指向内核路径：

1. **并行**：调度器支持多个开放槽位并发执行，没有波次屏障。`StaleVersion` 重提、迟到结果被拒，按 S6、S17、S18 处理。并行模块作用域扩展已由用户批准（2026-10-19）：入口、激活、成员 passes 与嵌套计数清零由纯函数 parallel_frontier 裁决；同一并行激活内只首次从外部进入时计数，后续激活重入递增；上限遵循 S17/S18。
2. **Op.call**：
   - 宿主代码提交 `CallSubmission::Bound` 后，Run 进入 `Waiting::Child`；
   - 子 Run 结算后，宿主代码提交 `resume` 和 `Completed` / `Failed` / `Uncertain`；
   - detach 模式按校验器已有语义处理；
   - 子 Run 同样走内核路径。

   谁观察子 Run 结算、谁发出释放，由你设计，但必须是宿主代码，不能是校验器。
3. **宿主操作与常驻助手**（上位计划 H4）：
   - `Yielded` / 打断，用 Code 提交带 `interruption` 的 NodeSubmission（D4、S14）；
   - 执行者冻结 `ArtifactKind::Interruption` 产物；
   - suppressed 与投递账本语义保持不变；
   - 必须让 `goose_persistent_assistant` 相关确定性测试在内核路径上通过。环境不具备时如实写"未执行"。
4. **执行结果补全**：
   - `BudgetExhausted`：Code 带 Observation 提交 stop，进入 `waiting / human / reason=budget`，人可以 resume。**不得**直接结算为失败，那是待决的 W4。
   - `Aborted`：人放弃。
   - `Interrupted` / `Suspended`：按 P 的取消路径处理。
5. **未知效果的人工决定**：现有 attempt 恢复决定 API（`record_recovery_decision`）接到内核路径。人核查后：
   - 选"已完成"：执行端口写入完成事实，然后 resume；
   - 选"放弃"：abandon。

   **不得自动重放。**
6. **删除 Run / Graph**：`deletion.rs` 对内核 Run 的支持。前置条件改为读相位与 worker。

**对照**（原 H2.2 中取消的那部分，现在值得做了）：并行组与调用组的随机生成器各取 5,000 个 seed，用同一个 fixture 端口分别交给旧 Runner 和新调度器执行：

- 比较派发多重集与最终 Graph 事实；
- 失败结局用 H2 任务书 §1 的三条不变量判定；
- 串行组已在 P 中做过 1,000 seed，扩到 10,000。

**M2 完成标准**：内核路径上没有 `Unsupported`；三组对照全绿；旧路径测试照旧全绿。

## 3. M3：默认切换到内核路径

1. 所有新 Run 的准入都走内核路径：删除 `execution=kernel_probe` 开关，删除"按存储归属分派"的双路径逻辑。
2. 磁盘上的旧 RunStore 记录在分支上不读取：按 C6，读到时具名拒绝，并提示需要离线迁移。生产数据迁移不在本里程碑。
3. **测试夹具迁移**：现有测试中用 `store.save` 直接构造旧 Run 的夹具，改为通过内核创建，必要时用 Code 提交把 Run 推进到所需相位。不得为了构造夹具而直接写内核存储文件。
4. 所有控制入口（control、abandon、recovery、session_calls、assistants、dispatch、admission、wecom 等）改为通过内核提交与读取相位。
   - 每个原先手写的前置条件：能由内核定律推出的，删除；属于 Session 层的，保留在 Session 层。
   - 每个文件写清删了哪些判断、保留哪些及理由。
   - S4（退役之后能否 abandon）按内核定律处理：未结算就可以 abandon，登记为 S 候选。
5. **真实 Goose 确定性回归**按 `docs/runtime-contract-tests.md` 执行一次。环境不具备时写"未执行"，这一项会成为合并前的门槛。

**M3 完成标准**：生产代码中没有任何路径构造 `GraphRunner`；全部测试全绿；C2 棘轮（H1 基线 16 处 save、36 处 mutation）降为 0，逐项列出每处的去向。

## 4. M4：删除旧实现

删除 `runner.rs`、`parallel.rs`、`RunStore` / `FileRunStore`、`RunControl` 的轮询接口、`RunStatus`、`RunCursor`、`CompletionFact` 中只为旧 Runner 存在的部分、R/R2 的临时适配与 `run_json` 兼容桥，以及其他只为它们存在的类型（以实际可删为准）。

- `GraphRunRecord` 中不再由它拥有的字段一并删除，例如 `cursor`、`status`、`error`、`recovery`、`recovery_submissions`。
- 差分测试失去了对照组：先用 M2 的对照生成 20,000 例随机 Graph 的派发轨迹与最终事实快照，提交为回归基线；然后差分测试改为对快照比对。
- 每个被删测试按 §0 的规则登记。

**M4 完成标准**：`grep -rn "GraphRunner\|RunStatus\|FileRunStore\|stop_requested\|pause_requested" rust --include=*.rs` 为零；全部测试全绿。

## 5. M5：测量与收尾

1. 接入测量探针（上位计划 H6）：Host 给内核注册 `Probe`，把每个测量点写入只读 JSONL 投影，不新增对外 API。
2. 写 `experiments/constitution-host-preflight/final/measurement.md`：
   - 按 crate 列出删除与新增行数（相对 `fc558e2`）；
   - 每条生命周期规则的独立判断点，迁移前后对比；
   - 直接写 Run 的点：前后对比；
   - 测试数量变化及登记表汇总；
   - 仍然手写的判断，按 Session 层、授权、执行端口归类。
3. 更新工作记录、上位计划状态行，以及合并前需要的事项清单：生产数据离线迁移、W4、真实 Provider 验收。

## 6. 约束

- C1：`anchor-kernel` 不改生命周期或业务规则。用户已明确批准唯一存储例外：FileStore::remove、删除恢复与单资源互斥；Store trait 不变，MemoryStore 不增加删除。其余内核改动仍属于"必须停下"。
- C2：Run 的状态只经 `Kernel::submit` 改变；M3 结束时验收为零。
- C3：校验器纯函数。
- C4：零停止 / 暂停轮询。pause 是宿主意图，由调度器在边界提交 stop，**不是**在执行循环中轮询标志。
- C5：身份与成本由宿主盖章，Agent 不得提交 Settle、fanout、join、interruption。
- 宪法第二条：未知效果不自动重放。
- 单一权威：任一时刻每个 Run 只在一个存储里。

## 7. 验收命令（每个里程碑交回前跑）

```bash
cd /home/mansteinl/code/anchor-constitution-core/rust
cargo build -p anchor-sandbox-bwrap --bins
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
git diff 6ee2b0e --stat -- rust/anchor-kernel    # 仅允许已批准的 FileStore 删除切片，Store trait / 生命周期不变
cd ../apps/web && npm test && npm run build
```

M1 起使用 `--workspace`，替代之前的六 crate 清单。

## 8. 两级停止条件

**必须停下交回：**

- 需要改 `anchor-kernel`，包括新增 Reason、Waiter、Phase 变体，或给 Stamp、Store 加接口。
- 需要改校验器的裁决规则；新增 Code 提交种类除外，需要的话也要先登记理由。
- 业务行为断言只能放宽才能通过（见 §0）。
- 违反宪法：结算可逆、自动重放未知效果、从提交内容读身份或成本、双权威。
- 某项能力在内核路径上做不到，且不属于 Session 层可以承担的事。

**自行判断、登记后继续：**

- 状态词汇和旧 Runner 内部机制断言的修改。
- 旧行为向内核定律对齐造成的差异：登记为 S 候选，附现场。
- 并发调度中由提交顺序决定的差异，前提是满足 H2 任务书 §1 的不变量。
- 接口形状、文件组织、Web 文案。

登记位置：工作记录中每个里程碑一节，下设"S 候选"、"断言修改"、"删除测试映射"三张表。

## 9. 里程碑交回模板

```
里程碑：M<n>
commit 列表：<hash 标题>（每个一行）
测试：workspace <通过/失败/忽略>；Web test <…>；Web build <…>；与上次交回差异 <…>
对照（M2）：serial/parallel/calls 各 <seed 数，差异>
C2 棘轮（M3 起）：save <n>，mutation <n>
删除行数 / 新增行数（M4 起）：<…>
S 候选：<条数与一句话列表>
断言修改：<条数>；删除测试：<条数>，均已映射
真实 Goose：<执行并通过 / 未执行及原因>
clippy / fmt：<…>
C1：<…>
停止条件触发：<无 / 哪条及现场>
需要审查方决定的问题：<…>
```

## 10. 审查方补充（2026-10-19；M1 交回时开始生效）

本节依据 P、R、R2 的数据修正本任务书的验收口径与流程，**与前文冲突时以本节为准**。M1 进行中的工作不需要回头改，交回时按 10.1 补齐即可。

### 10.1 验收口径：按宪法，不按行数

- 每个里程碑验收时，逐条回答 `docs/anchor-constitution.md` 中"完美"的三个条件：
  - 新增或改动的组件能否从宪法推出，或者属于可整块替换的边缘；
  - 核心（`anchor-kernel`）是否需要改动；
  - 核心里有没有可以删的东西。

  同时过一遍第 7 节的四个极限。行数只作辅助信号：新增显著多于删除时说明原因，不作为通过与否的条件。
- **度量口径改为宪法第 10 节的"同一规则的独立判断点"**，不再统计消费者函数包络。R、R2 统计的是包络，把 Session、授权、投递规则和 Run 规则混在一起，这和宪法的标准不是一回事。
- 每个里程碑交回时，附一张"规则清单"表，至少覆盖以下规则：
  - 是否终态
  - 可否 stop、resume、abandon、pause
  - 是否静止
  - 是否需要关注
  - 迟到结果如何处理
  - 未知效果如何处理
  - 预算耗尽如何处理
  - 子 Run 结算后如何释放父 Run

  每条规则列出：Host 中独立决定它的位置（`文件:行`），改前与改后各几处。凡是"读同一个视图或派生函数得出结论"的，不算独立判断点。
- **M1 交回时补测**：用同一张规则表，对 `1d1ba54`（R 之前）与 M1 完成后的代码各测一次。这一项替代 R2 的包络结论，作为读取侧是否统一的证据。

### 10.2 与 master 同步

- M1 交回后、M2 开始前，把 `master` 合并进 `constitution-core`，用 merge，不用 rebase。冲突解决逐文件登记在工作记录中。
- 之后每个里程碑结束时都再同步一次。同步后全量测试必须全绿，才算这个里程碑交回。

### 10.3 Goose 门槛

- 合并 master 之后，分支使用 master 的 Goose 锁定版本（`17e065…`）。本机 `/var/tmp/anchor-goose-build/artifacts/goose-acp.first` 与它一致；使用前用 `sha256sum` 核对。
- 从 M1 交回开始，每个里程碑都要跑依赖 Goose 的忽略测试，例如：
  ```bash
  ANCHOR_GOOSE_BINARY=/var/tmp/anchor-goose-build/artifacts/goose-acp.first \
    cargo test -p anchor-runner-host -- --ignored
  ```
  原样贴出各 target 的结果汇总。失败的必须定性：是 master 上原有的问题，还是迁移引入的。
- "未执行"只在二进制确实不可得时允许，并写明你查找过的位置。只查 PATH 不算查过。

### 10.4 测量探针提前到 M2

- 原 M5 的第 1 项（Host 为内核注册 `Probe`，测量点写入只读 JSONL 投影）移到 M2，最早在并行与 Op.call 落地之后接入。
- M2 交回时附一段真实运行的测量点样本，以及一个简短统计：按校验器、身份、动作、裁决、原因码汇总。M2 结束时由用户决定是否继续 M3。

### 10.5 Session 层"一个事实一个写入者"检验（M3）

M3 迁移控制入口时，列出 Session 层所有"读取 Run 状态来做业务决定"的位置，例如"Run 结束了，就投递回复"。逐条说明它在迁移后属于哪一种：

- 读取 Run 的结算事实（允许）；
- 应当成为 Session 自己拥有的事实；
- 两个写入者在写同一个事实（需要修正）。

最后一种按"必须停下"之外的规则登记并给出修正方案。只有修正需要改内核时，才停下交回。

### 10.6 证据规范

从 M1 交回起，不再提交原始 `.log` 文件。只提交摘要（`verification.md`、`measurement.md`）和机器可读的 `checks.json` / `counts.json`。原始输出放在 `.local/` 或 `/tmp` 下，路径写进摘要。已提交的 64 个原始日志暂不处理，合并到 master 之前再决定是否清理。

### 10.7 与 master 门面配合（2026-10-19，用户批准）

master 将按 `/home/mansteinl/Anchor/docs/run-access-facade-task.md` 建立 Run 访问门面，分三步：F0 围栏、F1 读取门面、F2 写入门面。业务代码只通过门面读写 Run，master 上行为零变化。本分支据此调整：

- **门面是契约。** M1 交回后合并 master，把门面带进来。之后本分支实现**同一套门面接口**，背后换成内核：
  - R2 的 `RunView` 与新对外词汇，改为门面在内核下的实现和序列化；
  - P 的 `kernel_runs` 并入门面实现；
  - 门面的每个规则谓词，在内核下按宪法定律给出新语义。与 master 语义不同的，逐条登记为 S 候选。
- **围栏随合并生效。** 本分支同样必须让 `run_access_fence` 保持全绿。分支新增的内核访问代码只能放在门面模块或运行时侧。
- **提前删除旧类型，"切换即删除"。** M3 只有在 master 的 F2 合并进本分支之后才能开始。M3 在同一个里程碑内完成两件事：
  1. 默认切换到内核；
  2. 删除门面的旧实现，以及 `GraphRunner`、`parallel.rs`、`RunStore`/`FileRunStore`、`RunStatus`、`RunCursor`、`RunControl` 中的轮询接口。

  此后 master 上若再有新代码直接使用这些类型，合并时会**编译失败**，冲突一律显式暴露。原 M3 中"所有控制入口改为内核提交"一项，因为业务代码已经只调用门面，范围缩小为门面实现本身。
- **M4 缩小为收尾**：删除 `GraphRunRecord` 中不再拥有的字段、`CompletionFact` 中只服务旧 Runner 的部分；差分测试改为对快照比对（做法同第 4 节）。
- 如果 F2 晚于本分支的 M2 完成，M2 结束后先做 M5 中的测量部分和 10.5 的检验准备，**不得**在没有门面的情况下提前开始 M3。

### 10.8 合并顺序（2026-10-19 补充）

master 的读取门面正在分片推进：F0 是 `e3a5778`，F1 的第一个切片是 `06f7b8a`，F1 还没做完。现在两边各有一套读模型，分别是 master 的 `application/run_access` 和本分支的 `run_view.rs` 等文件。为避免同一批读侧文件反复冲突，按下面的顺序合并：

1. 当前对 `2b3b7a2` 的合并照常完成并提交。
2. 接着**只合并 `e3a5778`（F0）**。合并后围栏会因为本分支新增的文件失败。处理方法：把 `application/kernel_runs.rs`、`kernel_runs/`、`run_view.rs`、`legacy_run_view.rs`、`run_json.rs`、`run_observations.rs` 移到 `application/run_access/` 下面。围栏已经把这个目录认定为门面，移过去就能通过。**不得**扩大 `runtime_file` 的豁免范围，也不得调高清单计数。这样 M2 期间新写的内核访问代码会受到围栏约束。
3. **master 的 F1 完成之前，不合并 `06f7b8a` 及之后的提交。** F1 完成后一次性对齐：
   - 以 master 的 `run_access` 接口为准，包括谓词名称和构造方式，本分支的内核实现放在这套接口后面；
   - 谓词在内核下的语义，按已批准的 S19–S24、S26 执行；
   - 预计 `application/run_access.rs` 会出现 add/add 冲突，合并时以 master 的接口为准。
4. 步骤 2 完成后即可开始 M2，不需要等 F1。

### 10.9 Goose 基线（G0 已交回并验收，2026-10-19）

- **根因**：Goose 测试的 fixture 没有配置 relay。任务联网用 slirp 隔离网络命名空间，访问不到宿主 loopback 上的 MCP 控制面，于是 ACP 在 `extensions/add` 处返回 `-32603`。生产与开发启动路径（`scripts/dev.sh`）本来就配置了 relay，不受影响。Goose 二进制不需要更换。修复提交为 `250435d`（分支 `fix/goose-baseline`），由 master agent 择机合入 master。
- **标准命令**：以 `docs/runtime-contract-tests.md` 为准（G0 合入 master 之后），关键是必须设置 `ANCHOR_GOOSE_LOCAL_NETWORK=1`，并让 `ANCHOR_GOOSE_RELAY_BINARY` 指向**同一次构建**产出的 `anchor-net-relay`。不设置这两项跑出来的 Goose 结果一律作废。
- **master 基线**（基点 `e3a5778`）：完整忽略测试 116 通过 / 3 失败 / 0 忽略。三项残留失败：
  1. `goose_conversation::next_conversation_invocation_changes_model_and_endpoint_without_losing_history`
  2. `goose_distribution::extracted_goose_runtime_resumes_without_sources_or_replaying_workspace_effects`
  3. `goose_pilot_authoring::candidate_declaration_cannot_enable_network_or_credentials_and_command_cancel_can_continue`
- **本分支的门槛**：在合并后的分支上用标准命令跑一次，失败名单不得超出上述三项。超出的每一项必须单独复跑确认，再定性为迁移引入还是环境原因。**不再需要**在 master-base worktree 上另跑 Goose 基线；已经跑过并且配置了 relay 的结果可以保留作为对照，没有配置 relay 的结果作废。

### 10.10 M1.5 审查结论与 M2 开工（2026-10-19）

- **M1.5 通过。** 审查方核对了以下几项：
  - 围栏测试与 master F0 逐字节一致，未扩大豁免，上限从 224 降到 192；
  - 内核零 diff；
  - 定向重跑全部通过：围栏、run_access / JSON 字节 / kernel_runs 17 项、Kernel 37 项。

  全量 workspace 和 Goose 这次没有重跑，因为与 master 的 F1 构建同时进行；M2 交回时一并重跑。
- **Goose 门槛已经起作用**：第一次带 relay 跑全量时，发现了 5 个 M1 词汇迁移遗漏，其中助手回执 2 个、Session Call 轮询 3 个，均已修复。这 5 个问题的测试此前从未在本分支上执行过。
- G0 的原始证据现在位于 `/home/mansteinl/Anchor/.local/goose-baseline-20261019/`。
- `goose_distribution` 是否通过，取决于构建目录里的沙箱二进制是不是硬链接，属于随环境变化的 fixture 缺陷。它在本分支通过不代表已经修复，基线名单仍保留三项。
- **双路径并存期间，规则计数持平甚至上升是正常的。** 旧路径与内核路径并存时，每条规则暂时各有一份实现，收敛要到 M3（切换并删除）才兑现。M3 交回的目标是：
  - 每条 Run 规则在 Host 中最多只保留"门面中的一处派生"，外加逐条列明的 Session 或授权收窄；
  - 写入门禁全部委托给内核；
  - 未知效果与预算的处理，收敛为门面中的恢复操作，加上执行端口提供的事实。

  达不到的，逐条说明原因。
- **可以开始 M2**，范围是 §2 加上 §10.4 的测量探针。M2 结束时是用户决定继续还是停止的检查点。
