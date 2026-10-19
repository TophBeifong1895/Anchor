# M2 删除边界：需要存储公开操作

任务书 §2 第 6 项要求 Kernel Run/Graph 删除；§8 同时规定“需要改 anchor-kernel，包括……给 Stamp、Store 加接口”必须停下交回。此次并行模块授权只覆盖 GraphValidator 的模块规则，没有授权修改 Kernel。

## 现场

- `anchor-runner-host/src/application/deletion.rs` 的 `delete_run` 与 `delete_graph_with_lease` 仍明确拦住 Kernel Run。清理只删除旧 RunStore、工作区与产物，不能删除 Kernel 权威资源。
- `application/run_access/kernel_runs.rs` 持有 `Kernel<FileStore>`。Host 只持有存储根目录，不拥有 FileStore 的资源路径格式。
- `anchor-kernel/src/store.rs` 的公开 Store 操作只有 `load/create/commit/history/list`；FileStore 公开构造函数，不公开 `dir/encode`，没有删除操作。
- `Kernel::submit` 的 Write/Request/Execute/Settle 均不删除资源。abandon 只结算；之后 load/list 仍返回原资源。

因此，仅替换 Host 的前置条件无法完成删除。复制 FileStore 的私有路径格式会把存储规则放到第二个模块；从 Host 直接读取内部 resource.json 定位目录同样穿透存储边界。另加隐藏标记只能隐藏资源，并不提供原来的物理删除，且增加一份持久删除解释。此次均未实施。

## 建议的最小授权范围

只为具体存储类型新增公开操作，例如：

```rust
impl FileStore {
    pub fn remove(&self, id: &str) -> Result<(), StoreError>;
}
```

可保持 `Store` trait 不变；Host 当前使用具体 `FileStore`，无需新增 Kernel 生命周期、Reason、Phase、Waiter 或 Graph 规则。目录编码与删除原子性仍由 FileStore 唯一实现。存储方法不判断 Graph/Session 权限或 Run 相位。

批准后由 `application/run_access` 提供删除门面，在同一宿主执行锁内复查 phase/worker，复用现有 Session 投递、调用引用、对话继任与删除凭证保护；防止 drive/resume 与删除交错。Run 清理及 Graph 级联复用同一存储删除操作。复制现有删除回归到 Kernel 路径，覆盖工作中拒绝、停止/结算后的产品边界、引用保护、重启、幂等清理与存储错误；存储删除的崩溃与目录边界另测。

删除对象包括权威资源及其历史，必须由存储明确提供这一能力。原子移出可见集合后清理、崩溃后的清理约定，需要在这项公开存储操作内统一定义，不交给业务代码推测目录。

当前 `anchor-kernel` 相对 `6ee2b0e` 仍零 diff，没有为绕过该边界写新持久字段。M2 尚未完成，不开始 M3。
