# M2 删除边界与已批准的存储操作

当前结论：用户已明确批准 `FileStore::remove(&self, id: &str) -> Result<(), StoreError>`；该存储切片已实现并提交 `0859373`，Host 删除接入已提交 `da708cb`，原存储 §8 边界解除。Store trait、MemoryStore、Kernel 生命周期与 Graph 裁决均不扩充。

## 授权前的现场

任务书 §2 第 6 项要求 Kernel Run/Graph 删除；§8 同时规定“需要改 anchor-kernel，包括……给 Stamp、Store 加接口”必须停下交回。此次并行模块授权只覆盖 GraphValidator 的模块规则，没有授权修改 Kernel。

### 存储缺口

- `anchor-runner-host/src/application/deletion.rs` 的 `delete_run` 与 `delete_graph_with_lease` 仍明确拦住 Kernel Run。清理只删除旧 RunStore、工作区与产物，不能删除 Kernel 权威资源。
- `application/run_access/kernel_runs.rs` 持有 `Kernel<FileStore>`。Host 只持有存储根目录，不拥有 FileStore 的资源路径格式。
- `anchor-kernel/src/store.rs` 的公开 Store 操作只有 `load/create/commit/history/list`；FileStore 公开构造函数，不公开 `dir/encode`，没有删除操作。
- `Kernel::submit` 的 Write/Request/Execute/Settle 均不删除资源。abandon 只结算；之后 load/list 仍返回原资源。

因此，仅替换 Host 的前置条件无法完成删除。复制 FileStore 的私有路径格式会把存储规则放到第二个模块；从 Host 直接读取内部 resource.json 定位目录同样穿透存储边界。另加隐藏标记只能隐藏资源，并不提供原来的物理删除，且增加一份持久删除解释。此次均未实施。

## 已批准并实现的范围

只在具体存储类型新增公开操作：

```rust
impl FileStore {
    pub fn remove(&self, id: &str) -> Result<(), StoreError>;
}
```

`Store` trait 保持不变；Host 当前使用具体 `FileStore`，无需新增 Kernel 生命周期、Reason、Phase、Waiter 或 Graph 规则。目录编码与删除原子性仍由 FileStore 唯一实现。存储方法不判断 Graph/Session 权限或 Run 相位。

Host 删除将由 `application/run_access` 提供删除门面，在同一宿主执行锁内复查 phase/worker，复用现有 Session 投递、调用引用、对话继任与删除凭证保护；防止 drive/resume 与删除交错。Run 清理及 Graph 级联复用同一存储删除操作。复制现有删除回归到 Kernel 路径，覆盖工作中拒绝、停止/结算后的产品边界、引用保护、重启、幂等清理与存储错误；存储删除的崩溃与目录边界另测。

删除对象包括权威资源及其历史，必须由存储明确提供这一能力。原子移出可见集合后清理、崩溃后的清理约定，需要在这项公开存储操作内统一定义，不交给业务代码推测目录。

FileStore 先将资源目录 rename 到 `.deleting-<encode(id)>`，sync parent，再 remove_dir_all 并再次 sync parent。load 遇到遗留删除目录时幂等完成清理；正常目录同时存在时仅清理旧删除目录，保留正常资源。create/history/commit 使用同一清理函数，避免新创建与遗留目录交错。

所有单资源访问使用既有 `anchor-file-lease` 的独占文件租约，每次打开独立描述符，位于 `.locks/<sha256(id)>`；load 内部用不重复加锁的 history_at，避免自锁。锁文件独立留存，资源删除不回收它。list 只返回规范编码的资源名，排除删除目录和锁目录；带点、斜线的合法资源 ID 仍用原编码规则。

测试新增 7 项：正常删除含历史和目录、幂等与 InvalidId、仅删除目录的恢复、正常与删除目录并存的恢复、内部目录不遮蔽合法 ID、两线程 100 轮 load/remove（交替覆盖同一实例和不同实例）。定向 7 / 0 / 0；Kernel 全量 44 / 0 / 0；Kernel clippy 通过。原始输出 `/tmp/anchor-m2/file-store-{deletion,kernel-regression,clippy}.log`；命令与 SHA256 见 checks.json。最初编译漏写租约 Errno 到 std::io::Error 的映射，已修正并保留初次失败输出，未改业务断言。

内核变更仅限获批的 FileStore 存储操作、相关锁依赖和删除测试，Store trait 与 MemoryStore 原样；kernel.rs/model.rs/lib.rs 相对 6ee2b0e 仍零 diff。没有新增持久 Graph 字段。Host 删除接线已完成，见 [Host 删除](host-deletion.md)；完整 M2 尚未完成，不开始 M3。

长 ID 的锁名使用既有 SHA256 依赖，保持固定长度；删除目录探测遇到文件系统名称长度限制时按父目录确认不存在，不影响原合法资源读取。不存在的合法 ID（含 512 字节及转义后超长）删除仍幂等。若正常目录可存在、但加 `.deleting-` 后超出文件系统 NAME_MAX，按规定的 rename 返回 Io，原目录及内容保留；不修改原 ID 编码或静默换用另一删除格式。该边界已由新增测试覆盖。
