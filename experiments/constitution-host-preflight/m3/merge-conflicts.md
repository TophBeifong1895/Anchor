# M3 §10.18 合并冲突与查询审计

合并父提交为本分支文档批准提交 `376b256` 与 master `dfd3427`，采用 merge。业务行为以 master 为准，执行与恢复实现保留本分支 Kernel 路径；本次不修改 Session schema。最终验收另见 [合并交回](merge-verification.md)。

## 三处内容冲突

| 路径 | 处理 |
| --- | --- |
| `apps/web/src/ChannelSessions.tsx` | 保留 master 的 `recovery_blocked` 展示、原因与历史入口；运行状态继续使用 Kernel `executing` / `RunProjection`。对应新增测试用 waiting 夹具替代旧 status 字段，展示断言不变。 |
| `docs/architecture.md` | 合并 master 的页面保留、渠道账号规范化与查询边界，保留本分支 Kernel 唯一执行与门面契约；冲突段中的旧生命周期说明更新为 Kernel Phase。 |
| `rust/anchor-runner-host/src/application/run_access/projection.rs` | 保留本分支 `read_run` / `RunProjection::from_view` / 规则谓词实现；不引入 master 的旧 active 表、pause/cancellation 标志或 observed 映射。master 的资格核对改动经共享 `ensure_current_assistant` 和渠道适配器自动合入。 |

## 自动合并后的语义适配

- `application/channel_management/tests.rs` 的 master 锁回归原用旧类型、`record.status` 与旧 active 表。保留测试名、部署查询时锁已释放、历史实例不查询部署、当前实例可恢复及停用后不可恢复等断言；夹具改为门面内真实 Kernel create/stop，测试专用 `registry_is_unlocked` 直接观察同一 `KernelRuns.active.try_lock()`。
- `tests/goose_persistent_assistant.rs` 保留 master 新增 `host_restart_recovers_only_the_canonical_account_and_preserves_legacy_history` 全场景；旧 running/stopped/cursor 断言改为 Kernel Executing/Waiting(Human)/frontier。原生 Session、工作区继承、旧历史与旧产物不变、canonical 唯一恢复、零恢复模型请求和新消息复用等断言保留。固定 Goose 相比 §10.15 新增这一项。
- 新的账号别名、Session v9、入站/交接规则和 Pilot/Responses/Scheduler 创建幂等均直接采用 master 的业务实现。`anchor-platform-session` 与 `dfd3427` 字节一致。

## (a) `KernelRuns.active` 全路径审计

该字段私有；所有生产锁点仅在 `run_access/kernel_runs.rs` 及其 `pause`、`recovery`、`deletion` 子模块。以下是每组锁点的调用范围：

| 锁点 | 锁内工作与边界 |
| --- | --- |
| `ExecutionGuard::drop`、`active`、`wait_pause` | 仅移除、查询或克隆 worker 登记；Notify 在锁释放后使用。 |
| `wait_idle` 两次观察 | 仅克隆 finished Notify 和比较 Arc；锁在 `notified.await` 前已经释放。 |
| `create`、`register` | 删除预约复查、Kernel resource 创建/读取、Host 时间观察或 pause 意图、worker 登记；不读取 Session、部署或 Plugin。 |
| `control` | Kernel 读取/提交、worker 取消通知及 pause 清理；同步函数不 await，不做外部资格读取。 |
| `request_pause`、`PauseBoundary::before_dispatch` | Kernel 读取/纯相位投影/提交、pause 文件与事件；不读取 Session、部署或 Plugin，不 await。 |
| `reserve_deletion` | 只检查 worker/删除预约并读取 Kernel resource；删除前业务引用、Session 与资格检查均在返回预约后进行。 |
| `observe_recovery_attempts` | Kernel resource/history、纯 frontier 和 Code observation 提交，不读取 Session、部署或 Plugin。 |
| `recover_attempt` | Kernel resource/history、参数校验、既有原生 invocation fact 的持久决定和 Human 提交。生产 persist 闭包只调用 `PreparedExecution` → NodeHost → Goose `record_recovery_decision`，读取/写入原生 fact，不读取平台 Session、部署或 Plugin。`prepare_resume` 在进入该方法前执行。 |

结论：没有在持 `KernelRuns.active` 时读取 Session、部署或 Plugin 的路径，也没有持锁跨 `.await`。查询 `read_run` → `KernelRuns::view` 只在 `active()` 中短暂查询 worker，调用 Session 与资格核对前锁已经释放。保留的 master 回归在部署 snapshot 回调中直接验证此锁可用。

## (b) 查询插件正文哈希

原 master 的耗时路径是 `project_run` → `ensure_current_assistant` → 渠道资格 → snapshot → 完整 Plugin definition。本分支合并前同样通过共享渠道适配器触发该完整检查。本次采用 master 的修复：

1. `ensure_current_assistant` 先核对 Session current binding；已退役或交接的历史 Run 在读取部署之前返回冲突。
2. 当前实例的资格仍调用 `ensure_channel_session_eligible`，账号选择与部署前置条件保持。`ChannelManagementAdapter::snapshot/status` 使用 live `PluginMetadata` 和凭据授权/资源存在性检查，不哈希插件正文。
3. `definition` 的完整检查保留在配置、启用、启动等执行前路径；没有借查询优化放宽写前校验。

保留 master 的 `channel_observation_skips_code_reads_but_revalidates_live_authorization_and_resources`：inotify 证明 snapshot/status 不打开 gateway 正文、完整 definition 会打开正文；修改授权声明或删除资源立即令资格不可用。与锁回归均在 Kernel Host 测试二进制中执行。

## 不变量

`rust/anchor-kernel` 相对 `0859373` 零 diff；未移植旧执行类型、未提高围栏上限、未新增业务语义或 S 候选。S27 已批准并结清 S4。测试结果、逐 target 原样汇总与二进制身份由最终交回记录，本文件不把审计推断写成真实模型 Provider 验收。
