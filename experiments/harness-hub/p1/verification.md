# P1 内核变更：验证与审查请求

执行：2026-10-20，按 [任务书 §P1](../../../docs/harness-data-hub-tasks.md#p1-内核变更时间因果配置只读读取)，由主 Agent 直接实现。字段的实际形状和选择理由见 [总体架构 5.1](../../../docs/harness-data-hub-architecture.md#51-内核变更p1-已实现待独立审查)。

## 改动范围

| 文件 | 增 / 删 |
| --- | ---: |
| `rust/anchor-kernel/src/kernel.rs` | +46 / −12 |
| `rust/anchor-kernel/src/model.rs` | +68 / −4 |
| `rust/anchor-kernel/src/store.rs` | +74 / −13 |
| `rust/anchor-kernel/src/lib.rs` | +2 / −2 |
| `rust/anchor-kernel/tests/constitution.rs` | +8 / −3 |
| `rust/anchor-kernel/tests/recorded_facts.rs`（新增） | +374 |
| `rust/anchor-runner-host/src/application/run_access/kernel_probe.rs` | +2 / −1 |

- 改动前，`git diff 0859373 -- rust/anchor-kernel` 等于本次的工作区改动，说明 P1 之前内核确实零改动。
- Runtime、GraphHost、Web 都没有改动。Host 只改了探针重建时的字段映射。

## 不变量核对

- **裁决规则零改动**：`adjudicate`、`resolve_pending`、`charge`、`task_control` 和三个宪法 Validator 的判断分支都没有变。新字段只在构造 Stamp、Point、Creation 和人工批准记录时写入，没有任何地方读取它们来做判断。
  - `recorded_facts.rs::verdicts_do_not_depend_on_time_cause_or_configuration` 用两组不同的时钟、配置和因果引用，跑同一组提交序列，其中包括预算越界结算。结果是裁决序列和最终资源完全相同。
- **仍然只有一个写入口**：`submit` 的签名没有变；只新增了 `with_clock` 这一个构造选项。
- **没有新增 Reason、Phase、Waiter、Action 或 Store 种类。**
- **旧数据**：新字段一律可选。没有新字段、并且带有旧 `knobs` 的记录可以正常读取，读出来是"未记录"。现有的 `storage_regressions.rs` 和 `file_store_deletion.rs` 夹具仍然带着 `knobs`，并且通过。
- **允许改动的断言**：只有 `tests/constitution.rs` 的 `every_verdict_is_recorded_and_measured_including_refusals`，从全局 knobs 改为 Principal 携带配置。原来"第二个点的 knobs"这条断言改为"第一个点没有配置，第二个点是 `config-v2`"，覆盖面没有缩小。其他断言都没有改。

## 新增测试（`tests/recorded_facts.rs`，9 项）

1. 内核时钟给每条记录盖时间，包括被拒绝的记录；`seq` 保持顺序。
2. 因果引用和配置原样进入历史和测量点，包括未授权被拒的提交。
3. 创建时间由内核盖章，创建者自带的时间会被覆盖；配置摘要在修改、结算和重新打开存储之后都保留。
4. 内核为提交生成的 effect 资源，沿用该提交的时间和配置。
5. 人工批准后写入目标资源的那条记录，因果引用指向 pending 资源上的决定记录。
6. 旧格式的记录和资源读出来是"未记录"。
7. 不同时钟、配置和因果引用下，裁决完全相同。
8. `HistoryReader` 的结果和 `FileStore` 一致：
   - 读取前后，数据目录的完整文件树（路径和大小）完全不变；
   - 写入者正在追加的半行不返回；
   - 资源被删除后读出来是不存在。
9. `HistoryReader` 不会创建不存在的根目录。

## 门槛

在标准构建目录 `/tmp/anchor-goose-regression-target` 下执行，原始日志在 `/tmp/anchor-p1/`，不入库。

| 门槛 | 命令 | 结果 |
| --- | --- | --- |
| 内核 | `cargo +stable test -p anchor-kernel --locked --offline` | 原有 53 项加新增 9 项，全部通过 |
| workspace | `cargo +stable test --workspace --locked --no-fail-fast` | 133 个目标，**2,209 通过 / 0 失败 / 126 忽略**。M5 是 2,200 / 0 / 126，多出的 9 项正是新增测试 |
| 严格 Clippy | `cargo +stable clippy --workspace --all-targets --locked -- -D warnings` | 通过 |
| fmt | `cargo +stable fmt --all -- --check` | 通过 |
| 围栏 | `cargo +stable test -p anchor-runner-host --locked --test run_access_fence` | 1 / 0 / 0 |
| 标准 Goose 套件 | `anchor-devtools regression goose-fixture`（见 [确定性回归](../../../docs/runtime-contract-tests.md)） | 20 套，107 / 0 / 0，没有真实模型请求 |
| 完整固定 Goose | `cargo +stable test -p anchor-runner-host --locked --no-fail-fast -- --ignored --test-threads=1`，环境同 M4 的 `goose_full` | **41 个目标，123 / 0 / 0**，和 M4 一致 |
| Web | 未重跑 | API 投影没有变化：Point 不对外暴露，Principal 的新字段可选，并且为空时不序列化 |
| 措辞检查 | `scripts/check-sensitive-wording.sh` | 通过 |

Goose 二进制的 SHA-256 是 `17e065a7…`，入口会校验。

## 未覆盖的边界

- Host 还没有填写配置摘要和因果引用，现在一律为空，留到 P2。
- `reads.rs` 的只读磁盘路径仍然用 `FileStore::new`，它会创建目录和锁文件。改为 `HistoryReader` 不属于 P1 的范围，作为后续整理项登记。
- 时间不保证单调。系统时钟回拨时，`at` 可能倒退，顺序以 `seq` 为准。
- 没有做并发读写压力测试；只验证了"写到一半的行"这种情况。

## 请求独立审查

P1 改动了内核。按宪法"提议和裁决分离"的原则，应由另一个 Agent 或用户审查，通过后以本提交作为新的冻结基点（C1）。建议审查时：

1. 读 `git show <本提交> -- rust/anchor-kernel/src`，确认新字段没有出现在任何判断条件里：
   `grep -n "\.at\b\|config\|cause\|created" rust/anchor-kernel/src/kernel.rs`
   每一处都应该是写入，而不是条件判断。
2. 复跑 `cargo +stable test -p anchor-kernel --locked --offline`，确认 `recorded_facts` 的 9 项都通过。
3. 判断"配置随 Principal 走"和"创建事实作为资源上的字段"这两个选择是否可以接受。理由见总体架构 5.1。
4. 如果审查要求修改，把问题记在这份文件末尾；修改后重新冻结。
