# M2 并行 Call 组合边界

当前按迁移任务书 §8 交回：现有准入允许并行分支包含 `Op.call`；`parallel_frontier` 将该分支固定派发为 `DispatchKind::Work`，绕过 GraphCallPort 与 CallSubmission。旧 Runner 的并行执行也直接调用 NodeExecutionPort。这是现有组合缺口，串行 Call 切片不修改该裁决规则。已批准的并行模块作用域扩展和 FileStore 删除均不包含这项裁决变更。

最小诊断源：[parallel-call-diagnostic.rs](parallel-call-diagnostic.rs)。它通过真实 GraphRunRecord 准入，接受 fanout 事实，再读取 GraphValidator frontier。实际运行于独立 `/tmp/anchor-m2/parallel-call-diagnostic/` Cargo 项目，依赖仓库 anchor-runtime 与 anchor-kernel；原始输出保留于 `/tmp/anchor-m2/parallel-call-diagnostic.log`。

实际输出：

```text
node=call kind=Work branch=Some(0)
node=work kind=Work branch=Some(1)
admission accepted; Call branch dispatches as Work, so GraphCallPort and CallSubmission are bypassed
```

诊断断言确认了当前边界，不构成对该组合的产品验收。

## 建议的最小扩展

1. 在纯函数 `parallel_frontier` 中根据 Op 类型生成 `DispatchKind::Call`，保持现有模块入口、激活与成员计数的唯一裁决位置。调用身份与输入仍从该槽位派生。
2. 并行 Call 使用已有 Code 提交 Bound/Completed/Failed/Uncertain/AdmissionFailed。Completed 只推进对应分支；wait 复用现有 `Waiting::Child`，不新增 Kernel 相位、等待类型或第二份调用游标。等待期间已派发兄弟的事实继续提交，按内核裁决留史、计费；宿主只释放当前持久绑定的子 Run。
3. 宿主继续负责子 Run 准入、执行、结算观察与 resume；detach 按现有接受语义处理。多分支先后绑定与恢复均从持久调用身份核查，不重放未知效果，不以其他子 Run 的结算释放当前等待。
4. 新增混合 Work/Call、多个 Call、wait/detach、异序结算、失败/Unknown、等待期间迟到兄弟和重启用例。既有 5,000 seed 串行调用组与并行组保留；组合单独验证，旧 Runner 在此组合也绕过调用端口，不能把它当作正确行为的对照。Kernel 与既有业务断言不改。

该扩展修改既有并行槽位的类型及结果裁决，不属于“新增 Code 提交种类”的例外。任务书 §8 明确要求“需要改校验器的裁决规则；新增 Code 提交种类除外，需要的话也要先登记理由”时停下交回；故尚未实施以上扩展。其余已授权切片的验证不替代这项批准。
