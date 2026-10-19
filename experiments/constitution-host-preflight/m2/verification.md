# M2 验证与当前边界

M2 进行中。用户已批准并行模块作用域的四点扩展，该 GraphValidator 切片已实现并通过本地确定性验证；原停止条件已解除。Host 的 KernelScheduler 仍为串行探针，M2 并行调度、Op.call、助手、结果补齐、恢复决定、删除与 Probe 尚待实现。此处不作为完整 M2 交回结论。

## 并行模块规则

入口判定、激活计数、成员 passes 与嵌套模块计数清零都在 `parallel_frontier` 内，以与串行 prepare 一致的纯状态变换执行。首次从外部进入时计一次；同一并行激活中，已接受的分支进度证明该模块已进入，其他分支不会重复计数。后续 fanout 激活重新进入时增加计数。

每个候选槽位独立预演；查询 frontier 不保存激活，裁决只接受被提交槽位对应的模块变更。因此不同模块的慢分支不会被其他分支提前计数或反复清零。新增的是纯函数的提交槽位参数，没有新增存储字段或第二份执行游标。

模块上限沿现有 conclude 结算，并调用既有 refuse_module_activation 写下阻断事实；先接受的终结事实保持唯一，迟到的成功/失败均以 TaskSettled 留在历史并计费。Kernel 零改动，旧 Runner 及既有测试断言不变。现有旧 Runner 模块用例已复制到内核，另覆盖两次激活、共享模块延迟进入、不同模块异序完成、成员/嵌套计数重置、上限后的迟到结果与失败。

## 已执行验证

Rust 命令均在 `rust/` 下执行，原始输出保留在 `/tmp/anchor-m2/`，未提交日志。机器摘要与完整 Tally 见 [checks.json](checks.json)。

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
