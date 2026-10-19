# M2 验证与当前边界

M2 进行中。已批准的并行模块校验器与 Host 多槽位调度均已实现并通过本地确定性验证。FileStore 删除操作现已按用户批准实现并通过存储测试，原 §8 边界解除，继续 Host 删除与其余能力；见 [具体边界与最小授权建议](deletion-boundary.md)。Op.call、助手、结果补齐、恢复决定、删除与 Probe 尚未完成，不开始 M3。这不是完整 M2 交回。

## FileStore 删除存储切片

Store trait 和 MemoryStore 未改；FileStore 提供获批 remove，rename/sync/remove/sync，load 幂等清理遗留目录，资源锁文件单独留存。定向 7 项、Kernel 全量 44 项及 clippy 通过，详见 [存储操作与证据](deletion-boundary.md)。当前 Kernel diff 仅限获批存储切片与测试，不再声称整个 crate 零 diff；生命周期代码仍零 diff。Host Run/Graph 删除接线尚在进行。

## Host 多槽位调度

生产 KernelScheduler 使用 FuturesUnordered 管理开放槽位；每个已观察完成事实只提交一次，成功分支产生后继时可立即派发，不等待慢兄弟。fanout/join 由宿主 Code Write 提交，纯控制产物构造与旧 Runner 共用，Work 请求继续复用原授权构造函数。ArtifactFreezeContext 保留控制种类和固定输入来源，分支输入带既有 activation/branch 标识。

等待、结算、取消、端口错误或 panic 之后收集已派发任务；迟到成功和失败交由内核拒绝留史，StaleVersion 重提成本为零。调度器不计模块激活、不写 Graph 状态，也不新增停止/暂停标志轮询。Host 并行 pause 验证结果先提交、随后 Human stop(reason=paused)、不取消兄弟、draining 时不允许 resume、恢复不重放已知完成事实。

复制旧 Runner 的异序分支与 join、分支失败、共享模块重入用例到实际调度器；新增无屏障后继、迟到与停止恢复、端口错误/panic 收集用例。并行随机失败组直接复用原 S18 三条不变量检查器，原断言、seed、失败注入概率与阈值未改。新测试中的 fixture panic 是有意输入，捕获后仍提交兄弟事实，测试通过。

实际调度器与旧 Runner 使用同一 Scripted/Artifacts fixture：串行 10,000（772 节点失败、1,310 激活模块）、并行失败 5,000（775 节点失败、10,224 过期重提）、原无失败并行 5,000、模块 5,000（4,588 激活模块、12,461 过期重提）通过。模块组仍为无节点失败注入的补充；模块上限与迟到失败由定向用例覆盖。这里只完成串行/并行对照，实际调度器调用组尚待 Op.call 切片。

新切片实际命令在仓库根目录执行，完整命令、target 汇总与输出哈希见 checks.json，原始日志在 /tmp/anchor-m2/：

| 检查 | 结果 |
| --- | --- |
| `cargo test --manifest-path rust/Cargo.toml -p anchor-runtime --lib graph::scheduler -- --nocapture` | 9 / 0 / 0，含 3 组实际调度器随机对照 |
| `cargo test --manifest-path rust/Cargo.toml -p anchor-runtime -- --skip graph::scheduler::tests::differential` | 141 单元 + 7 集成 + 2 doctest 通过；含原校验器完整差分 |
| `cargo test --manifest-path rust/Cargo.toml -p anchor-runner-host --bin anchor-runner-host application::run_access::kernel_runs::tests -- --nocapture` | 15 / 0 / 0，含原 FileStore 1,000 seed 串行对照 |
| 同一 Host 命令仅过滤 `application::run_access::kernel_runs::tests::parallel` | 1 / 0 / 0，最终代码定向确认 |
| Runtime 与 Host 各 `cargo clippy --manifest-path rust/Cargo.toml -p <crate> --all-targets -- -D warnings` | 均通过 |
| `cargo test --manifest-path rust/Cargo.toml -p anchor-runner-host --test run_access_fence` | 1 / 0 / 0；围栏测试与 F0 字节一致，清单本切片未改 |

fmt、git diff --check、工程措辞检查通过；Kernel 相对 6ee2b0e 零 diff。新增 Runtime 9 项、Host 1 项，既有业务断言修改 0、删除 0、新增 ignore 0。Runtime 分两条实际命令覆盖全部 target，不合成为一次测试输出。

初次新 fixture 编译因 Runtime 未启用 tokio sync 失败，已补 dev feature；初次 Host 命令误用 --lib（该 crate 只有 bin），已改为 --bin。这两次临时日志被随后运行覆盖，原输出现不可获取，不计为通过证据。未新增完整 workspace、Web、Goose 或真实模型 Provider 验收；因 §8 边界交回，不宣称 M2 完成。

## 并行模块规则

入口判定、激活计数、成员 passes 与嵌套模块计数清零都在 `parallel_frontier` 内，以与串行 prepare 一致的纯状态变换执行。首次从外部进入时计一次；同一并行激活中，已接受的分支进度证明该模块已进入，其他分支不会重复计数。后续 fanout 激活重新进入时增加计数。

每个候选槽位独立预演；查询 frontier 不保存激活，裁决只接受被提交槽位对应的模块变更。因此不同模块的慢分支不会被其他分支提前计数或反复清零。新增的是纯函数的提交槽位参数，没有新增存储字段或第二份执行游标。

模块上限沿现有 conclude 结算，并调用既有 refuse_module_activation 写下阻断事实；先接受的终结事实保持唯一，迟到的成功/失败均以 TaskSettled 留在历史并计费。Kernel 零改动，旧 Runner 及既有测试断言不变。现有旧 Runner 模块用例已复制到内核，另覆盖两次激活、共享模块延迟进入、不同模块异序完成、成员/嵌套计数重置、上限后的迟到结果与失败。

## 先前校验器切片的已执行验证

下表校验器切片的 Rust 命令在 `rust/` 下执行，原始输出保留在 `/tmp/anchor-m2/`，未提交日志。机器摘要与完整 Tally 见 [checks.json](checks.json)。

| 检查 | 结果 |
| --- | --- |
| `cargo test -p anchor-runtime --lib parallel_module -- --nocapture` | 5 通过：4 个定向用例，加 5,000 模块随机样本 |
| `cargo test -p anchor-runtime --lib graph::validator::differential -- --nocapture` | 6 通过；串行 10,000、原并行失败 5,000、原并行无失败 5,000、调用 5,000、新模块 5,000；S17/S18 独立 seed 通过 |
| `cargo test -p anchor-runtime -- --skip graph::validator::differential` | Runtime 其余 129 单元测试、7 集成测试、2 doctest 通过 |
| `cargo clippy -p anchor-runtime --all-targets -- -D warnings` | 通过 |

三组差分与 Runtime 其余测试是分开的实际执行，不拼接为一次命令。原并行失败组仍为 775 个节点失败、6 个多失败波次、2 个上限先结算后收到失败；原无失败组仍为 5,048 次并行激活，原 seed、注入概率与阈值未改。

当前分支的 `random_parallel_graph` 实际会清空 `module_rounds`，分支节点也无模块前缀，去掉准入拒绝不会自动形成模块覆盖。因此保留原生成器和原阈值，新增 5,000 个共享/独立模块样本：4,588 例激活模块、5,084 次并行激活、12,461 次过期重提，全部通过。模块样本不注入节点失败、模块上限设在有限 fanout 激活次数之上；模块上限与迟到失败由定向用例验证，不把该样本说成覆盖了随机模块失败。

初次新定向测试曾错误地比较迟到前后的完整 Resource，忽略 D1 要求拒绝提交也计费；已修正为验证 phase、version、Graph state 不变且 spent 增加实际成本 3。既有断言未放宽。

workspace、Web、Goose 与真实模型 Provider 尚未在 M2 新增验收；M2 结束时仍按任务书执行全部要求。

## 开工时的边界（已获批准并实现）

同一份合法 Graph：`start` fanout 到 `module/left` 与 `module/right`，经 `collect` join 后由 `after` 反馈到 `start`；`module_rounds["module"]=1`。

- 旧 Runner：两条分支各执行一次，`module_activations["module"]=1`；重入时产生 `module@1`，并按模块上限失败。这是预期的边界结果。
- Kernel 入口：`run_resource` 在创建资源之前返回 `Unsupported("module scopes inside parallel branches are not admitted by the Graph validator yet")`，未开始任何执行。
- 既有测试 `graph::tests::parallel_region_counts_module_activation_once_and_enforces_reentry_ceiling` 独立通过。

代码入口是 `rust/anchor-runtime/src/graph/validator.rs:179` 的准入拒绝、`validator.rs:372` 的 `parallel_frontier`，以及旧路径 `parallel.rs:324` 的模块计数/上限规则。现有 `parallel_frontier` 只处理节点轮数上限，不处理模块激活、模块上限及成员 passes 的重置。仅去掉准入拒绝不能恢复旧路径已有能力；把这些事实写入 Host 调度器也会绕开 Graph 的唯一裁决者。

## 原审查建议（四点已获用户批准）

建议批准仅扩展 GraphValidator 的并行模块作用域规则：

1. 同一并行激活中，同一模块被多个分支进入时只计一次激活；模块重入增加计数并执行既有模块上限。
2. 模块入口、成员 passes 与嵌套模块计数重置在纯校验逻辑内完成，不依赖旧 cursor、波次屏障或执行者自述。
3. 模块上限触发时沿已批准 S6/S17/S18 的首次终结事实语义结算；后到事实仍入拒绝历史，成本不丢失。
4. 复制旧模块用例到内核路径，覆盖共享模块、不同模块、重入上限、无波次完成顺序与迟到提交；并行随机对照增加模块样本。Kernel 保持零改动，校验器保持纯函数。

这是现有节点提交的准入与状态裁决扩展，不属于新增 Code 提交种类。该边界在初次开工时按任务书 §8 交回，现已获用户明确批准；以上停止结论只描述修复前的诊断。尚无新增 S 候选、断言修改或删除测试。

## 修复前的诊断验证

| 命令（在 `rust/` 下） | 结果 | 原始输出 |
| --- | --- | --- |
| `cargo test -p anchor-runtime --features test-fixtures --test m2_parallel_scope_boundary -- --nocapture` | 1 通过；验证能力缺口存在，不代表 M2 实现通过 | `/tmp/anchor-m2/parallel-module-boundary.log` |
| `cargo test -p anchor-runtime --lib graph::tests::parallel_region_counts_module_activation_once_and_enforces_reentry_ceiling -- --exact --nocapture` | 1 通过 | `/tmp/anchor-m2/old-module-test-exact.log` |

首次使用未限定路径的 `--exact` 过滤器选中 0 个测试，不计为通过证据；该输出保存在 `/tmp/anchor-m2/old-module-test.log`。随后用完整测试名执行了上表中的独立测试。机器摘要与原始输出 SHA256 见 [checks.json](checks.json)。

初次诊断未执行 workspace、Web、Goose 或真实模型 Provider 验收，且没有生产代码改动。M2 完成时仍须执行任务书规定的全量验证与三组对照。

## 复现源代码

下面是本次实际运行的完整临时测试源码。执行时复制到 `rust/anchor-runtime/tests/m2_parallel_scope_boundary.rs`，执行后已移除该临时副本；源码原件在 `/tmp/anchor-m2/parallel-module-boundary.rs`。按 §10.6 只提交摘要与机器证据，原始日志不入库。

```rust
use anchor_runtime::graph::{
    GraphEdge, GraphError, GraphNode, GraphRunRecord, RunStatus,
    test_support::{random_graph, run_old},
    validator::run_resource,
};
use serde_json::{Value, json};

#[tokio::test]
async fn m2_parallel_module_boundary_reproduces_old_support_and_kernel_refusal() {
    let mut snapshot = random_graph(0);
    snapshot.entry = "start".into();
    snapshot.module_rounds.clear();
    snapshot.module_rounds.insert("module".into(), 1);
    snapshot.ops.clear();
    snapshot.ops.insert("fanout".into(), json!({"fanout":{"join":"collect"}}));
    snapshot.ops.insert("join".into(), json!({"join":{}}));
    snapshot.nodes = ["start", "module/left", "module/right", "collect", "after"]
        .into_iter()
        .map(|id| {
            let op = match id {
                "start" => Some("fanout".into()),
                "collect" => Some("join".into()),
                _ => None,
            };
            GraphNode {
                id: id.into(),
                agent: op.is_none().then(|| "worker".into()),
                op,
                input: None,
                plugins: vec![],
                max_rounds: None,
            }
        })
        .collect();
    snapshot.edges = [
        ("start", "module/left"),
        ("start", "module/right"),
        ("module/left", "collect"),
        ("module/right", "collect"),
        ("collect", "after"),
        ("after", "start"),
    ]
    .into_iter()
    .map(|(from, to)| GraphEdge { from_node: from.into(), to_node: to.into() })
    .collect();
    snapshot.validate().unwrap();
    let record = GraphRunRecord::create_with_id(snapshot, Value::Null, "m2-module-boundary").unwrap();
    let error = run_resource(&record, u64::MAX).unwrap_err();
    assert!(
        matches!(&error, GraphError::Unsupported(message)
            if message == "module scopes inside parallel branches are not admitted by the Graph validator yet"),
        "{error}"
    );
    let (trace, old) = run_old(record, 0, false).await;
    assert_eq!(old.status, RunStatus::Failed);
    assert_eq!(old.module_activations.get("module"), Some(&1));
    assert!(old.ceased.contains("module@1"));
    for node in ["module/left", "module/right"] {
        assert_eq!(trace.iter().filter(|(id, _, _)| id == node).count(), 1);
    }
    println!("{}", json!({
        "scenario": "same module in two parallel branches; reentry ceiling 1",
        "old": {
            "status": old.status,
            "module_activations": old.module_activations,
            "ceased": old.ceased,
            "trace": trace
        },
        "kernel_admission_error": error.to_string(),
        "capability_implemented": false,
        "stop_condition": "constitution-migration-task section 8: validator judgement rule change"
    }));
}
```
