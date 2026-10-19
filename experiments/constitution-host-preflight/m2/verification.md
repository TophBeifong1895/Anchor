# M2 开工：并行模块作用域边界

状态：按任务书 §10.10 开始 M2，尚未完成任何 M2 能力切片。在实施并行项前复核到 §8 的必须停止条件，现交回审查。生产代码、校验器、Kernel 与既有测试均未修改；旧 Runner 仍为默认入口，未合入 F1。

## 已确认的现场

同一份合法 Graph：`start` fanout 到 `module/left` 与 `module/right`，经 `collect` join 后由 `after` 反馈到 `start`；`module_rounds["module"]=1`。

- 旧 Runner：两条分支各执行一次，`module_activations["module"]=1`；重入时产生 `module@1`，并按模块上限失败。这是预期的边界结果。
- Kernel 入口：`run_resource` 在创建资源之前返回 `Unsupported("module scopes inside parallel branches are not admitted by the Graph validator yet")`，未开始任何执行。
- 既有测试 `graph::tests::parallel_region_counts_module_activation_once_and_enforces_reentry_ceiling` 独立通过。

代码入口是 `rust/anchor-runtime/src/graph/validator.rs:179` 的准入拒绝、`validator.rs:372` 的 `parallel_frontier`，以及旧路径 `parallel.rs:324` 的模块计数/上限规则。现有 `parallel_frontier` 只处理节点轮数上限，不处理模块激活、模块上限及成员 passes 的重置。仅去掉准入拒绝不能恢复旧路径已有能力；把这些事实写入 Host 调度器也会绕开 Graph 的唯一裁决者。

## 需要审查的最小扩展

建议批准仅扩展 GraphValidator 的并行模块作用域规则：

1. 同一并行激活中，同一模块被多个分支进入时只计一次激活；模块重入增加计数并执行既有模块上限。
2. 模块入口、成员 passes 与嵌套模块计数重置在纯校验逻辑内完成，不依赖旧 cursor、波次屏障或执行者自述。
3. 模块上限触发时沿已批准 S6/S17/S18 的首次终结事实语义结算；后到事实仍入拒绝历史，成本不丢失。
4. 复制旧模块用例到内核路径，覆盖共享模块、不同模块、重入上限、无波次完成顺序与迟到提交；并行随机对照增加模块样本。Kernel 保持零改动，校验器保持纯函数。

这是现有节点提交的准入与状态裁决扩展，不属于新增 Code 提交种类。任务书 §8 明确要求此时停下交回；本次没有实施该扩展，也没有以返回成功或缩小 Graph 能力范围绕过它。尚无新增 S 候选、断言修改或删除测试。

## 实际验证

| 命令（在 `rust/` 下） | 结果 | 原始输出 |
| --- | --- | --- |
| `cargo test -p anchor-runtime --features test-fixtures --test m2_parallel_scope_boundary -- --nocapture` | 1 通过；验证能力缺口存在，不代表 M2 实现通过 | `/tmp/anchor-m2/parallel-module-boundary.log` |
| `cargo test -p anchor-runtime --lib graph::tests::parallel_region_counts_module_activation_once_and_enforces_reentry_ceiling -- --exact --nocapture` | 1 通过 | `/tmp/anchor-m2/old-module-test-exact.log` |

首次使用未限定路径的 `--exact` 过滤器选中 0 个测试，不计为通过证据；该输出保存在 `/tmp/anchor-m2/old-module-test.log`。随后用完整测试名执行了上表中的独立测试。机器摘要与原始输出 SHA256 见 [checks.json](checks.json)。

本轮未执行 workspace、Web、Goose 或真实模型 Provider 验收；没有生产代码改动，不重复 M1.5 的全量检查。M2 完成时仍须执行任务书规定的全量验证与三组对照。

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
