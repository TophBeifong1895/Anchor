# P1 内核变更：验证与审查请求

执行：2026-10-20，按 [任务书 §P1](../../../docs/harness-data-hub-tasks.md#p1-内核变更时间因果配置只读读取)，由主 Agent 直接实现。字段的实际形状和选择理由见 [总体架构 5.1](../../../docs/harness-data-hub-architecture.md#51-内核变更p1-已实现第一轮审查后已修正待复审)。

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

## 独立审查（2026-10-20，审查 a9366c5）

结论：**暂不冻结为 C1**。裁决零改动这一点成立；但第 1 条问题会影响 `cause` 的含义，冻结前需要先处理。

已核对：

- 请求 1：`kernel.rs` 中 `at`、`config`、`cause`、`created` 的每一处都是写入，没有出现在任何判断条件里。Host 的 `RunAuthority` 和 `GraphValidator` 只读取 `principal.id` 和 `kind`，也不读新字段。
- 请求 2：`anchor-kernel` 共 53 项通过，其中 `recorded_facts` 9 项；本地复跑结果一致。
- 请求 3：两个设计选择都可以接受，附带的条件见下面第 6 条。

### 问题

1. **（必须处理）`seq` 在内核重启后从 1 重新计数。** `writer: Mutex::new(0)`（`kernel.rs:81`），而且启动时不会从存储里恢复。我在仓库外写了一个测试，在同一个 FileStore 上重启内核，`run/1` 的历史 seq 是 `[1, 1]`。后果有三点：
   - `Cause { resource, seq }` 在跨重启的历史里会有歧义；
   - 本文"顺序以 `seq` 为准"这句话跨重启不成立；
   - 根据代码推断（没有实际运行验证）：`effect/{target}/{seq}` 和 `pending/{target}/{seq}` 重启后可能撞名。`store.create` 返回 `Exists` 时，目标记录已经提交了。生产 Host 目前不用 Execute 和 Pending，所以还没有触发。
   - 建议：`Kernel::new` 时从存储恢复 seq 的最大值，或者把计数器持久化。记录格式不需要改。
2. **（应当处理）`with_clock` 在 `new` 之后才生效。** 所以三个宪法 Validator 资源，以及在 `with_clock` 之前 `install` 的资源，`created.at` 总是取自 SystemClock。实测：注入固定时钟 42 后，`validator/constitution` 的 `created.at` 仍然是真实的墙钟时间。`recorded_facts.rs:46-49` 的测试辅助函数本身就是这个顺序。建议让时钟能在构造时传入。
3. **（小）`HistoryReader::load` 和删除并发时会违反文档。** 如果它读到 `resource.json` 之后，资源目录被改名删除，接着读历史会得到 NotFound，被当成空历史处理，结果返回 `Some(resource)`，而且 `spent = 0`。文档承诺的是"被删除的资源读出来就是不存在"。建议在组装完成后再检查一次 `resource.json` 是否还存在，不存在就返回 `None`。
4. **（小）文件名超长时，`HistoryReader::load` 和 `FileStore::load` 行为不一致。** 资源 id 编码后超过 NAME_MAX 时，`fs::read` 返回 InvalidFilename，`HistoryReader::load` 把它当错误返回；`FileStore::load` 用的是 `exists()`，会返回 `None`。这条根据代码推断。
5. **（小）人工批准记录的 `cause` 指向一条稍后才写入的记录。** `resolve_pending` 先提交目标记录，之后 `submit` 才提交 pending 资源上的决定记录。如果在两次提交之间崩溃，`cause` 就悬空了。建议在 Cause 的文档里写明：引用可能悬空，读取方必须容忍。
6. **（设计附带条件）`config` 放在 Principal 上之后，`Principal` 的 `PartialEq/Eq` 也会比较 config。** 以后如果有 Authority 或 Validator 用整个 Principal 做比较或者当作键，config 就会悄悄变成身份的一部分，进而影响裁决。建议在 `Principal.config` 的文档里写明：授权只能按 `id`、`kind` 判断。
7. **（测试缺口）** `verdicts_do_not_depend_on_time_cause_or_configuration` 的两次运行用的是同一组 `cause("x", n)`，因此"不同因果引用"实际上没有变化。另外，这个差分测试没有覆盖 Pending/批准路径和 Execute/effect 路径。建议让两次运行的 cause 不同，并补上这两条路径。

第 1 条和第 2 条修改后，请重跑内核测试和 workspace 门槛，再申请冻结。

## 对第一轮审查的处理（2026-10-20）

| # | 处理 | 验证 |
| --- | --- | --- |
| 1 | 构造内核时，从存储的所有历史里恢复最大的 `seq`，作为写入计数器的起点（`kernel.rs` 的 `last_seq`）。记录格式不变。启动代价是线性读一遍历史 | 新测试 `seq_continues_across_a_restart_so_names_and_causes_stay_unique`：在同一个 FileStore 上重启内核，`run/1` 的 seq 是 `[1, 3, 4]`；重启前后两次 Execute 生成的 effect 资源名不同，共 2 个，没有 `Exists` 错误 |
| 2 | 去掉 `with_clock` 构造器方法，改为构造函数 `Kernel::with_clock(store, authority, clock)`；`Kernel::new` 等价于传入 `SystemClock`。测试辅助函数同步改为构造时注入 | 新测试 `the_clock_given_at_construction_stamps_the_constitutional_validators_too`：注入从 42 开始的时钟，三个宪法 Validator 和测试用 Validator 的创建时间依次是 42 到 45 |
| 3 | `HistoryReader::load` 组装完成后再检查一次 `resource.json`，不存在就返回 `None` | 这个时间窗口没法在测试里确定性地复现，属于代码层面的修复；删除后读取的路径由已有测试覆盖 |
| 4 | 读取侧把 `NotFound` 和 `InvalidFilename` 都当作"不存在"，和 `FileStore` 用 `exists()` 判断的结果一致 | 新测试 `an_id_too_long_for_the_filesystem_reads_as_absent_in_both_views`：编码后约 600 字节的 id，两个读取接口都返回"不存在" |
| 5 | 在 `Cause` 的文档里写明：`seq` 在同一个数据根下跨重启唯一；引用可能悬空，读取方必须能容忍 | 文档 |
| 6 | 在 `Authority` trait 和 `Principal.config` 的文档里写明：只能按 `id` 和 `kind` 判断，不能拿整个 `Principal` 做比较或者当作键 | 文档；复核了 Host 的 `RunAuthority` 和 `GraphValidator`，也只按 `id` 和 `kind` 判断 |
| 7 | 差分测试的两次运行，时钟、配置、因果引用都不同，并且覆盖了普通工作、Execute 和 effect 生命周期、Pending 和人工批准、过期提交、预算耗尽 | 8 个裁决和所有资源（去掉创建戳之后）完全相同；裁决原因依次是 `Ok ×4, AwaitingHuman, Ok, StaleVersion, BudgetExhausted` |

### 重跑的门槛

原始日志在 `/tmp/anchor-p1b/`。

| 门槛 | 结果 |
| --- | --- |
| 内核 | `recorded_facts` 12 项，内核测试全部通过 |
| workspace | 133 个目标，**2,212 / 0 / 126**（比第一轮多 3 项，正是新测试） |
| 严格 Clippy、fmt、围栏 | 通过；围栏 1 / 0 / 0 |
| 完整固定 Goose | **41 个目标，123 / 0 / 0** |
| 标准 Goose 套件 | 第一次在 `goose_completion` 有 1 项失败，原因是测试 Host 启动时端口被占用（`AddrInUse`），属于环境冲突；同一项在随后的完整 Goose 里通过。单独重跑整套：**20 套，107 / 0 / 0**。两次的证据都保留在 `/tmp/anchor-p1b/` |

### 对已有开发数据的影响

8077 现在运行的部署版本还是旧内核，不受影响。以后部署新内核时，旧开发数据里重启前留下的重复 seq 会保留在历史里不变，新的提交从最大值往后编号。旧记录本来就没有 `cause`，所以不会产生有歧义的引用。

请复审第 1、2 条，确认后冻结。
