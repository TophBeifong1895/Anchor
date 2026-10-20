# M4 GraphRunRecord 字段归属

审计基点：master 的任务书提交 2cef07b。这里只登记当前字段及所有者，不修改 Kernel、Graph 裁决、冻结快照或外部恢复行为。

| 字段 | 处置 | 所有者及理由 / 证据 |
| --- | --- | --- |
| format | 保留 | Graph schema 的解码边界拥有，不是通用 Resource 生命周期版本。当前 15；旧 1–14 和被删字段均具名拒绝。通用 Kernel 不认识 Graph 格式，不能替它增加规则。 |
| started | 保留 | Graph 初始准入的不可变时间观察，独立 Graph 宿主也可使用；不参与状态或更新时间判定。Host HTTP 的开始时间继续使用 metadata.created。 |
| updated | 删除 | Host RunObservations 的已接受提交观察拥有更新时间；无生产 touch 调用。 |
| run_id | 保留 | Graph 调用/产物身份，用于绑定 InvocationKey 与 Resource run/id。 |
| graph_digest | 保留 | Graph 冻结定义摘要，绑定调用、权限和产物，validate 校验 snapshot digest。 |
| snapshot | 保留 | Graph 校验器拥有冻结拓扑、路由、模块及节点策略。 |
| input | 保留 | Graph 准入合并后的有效输入，执行请求复用；不从当前作者定义重算。 |
| parallel | 保留 | Graph 校验器拥有 fanout 激活、分支进度与已提交前缀，不是 Kernel 任务相位。 |
| recovery | 删除 | 已接受 Kernel history 的 Code Observation 经 project_recovery 纯函数投影为 RunView.recovery；在线门面与只读磁盘入口共用。 |
| recovery_submissions | 删除 | 人工决定在 Kernel history，幂等查询直接使用 recovery_facts，不经 Graph 记录搬运。 |
| graph_calls | 保留 | Graph 调用身份、子 Run 绑定、模式和结果业务事实；validator judge_call 写入，宿主据此闭合子 Run。 |
| plugin_bindings | 保留 | Graph 准入冻结的 Plugin 权限事实，执行请求和调用授权使用。 |
| plugin_bindings_initialized | 保留 | 区分尚未完成 Plugin 准入与已完成但集合为空；authorized_execution_request 和 invocation_plugin_authority 使用，不能把空集合当已授权。 |
| invocations | 保留 | Graph 全局节点调用序号，用于当前 frontier、产物、Call 幂等身份。 |
| passes | 保留 | Graph 当前模块激活内节点 passes / round ceiling；模块重入可清零，与全局 invocation 不同。 |
| module_activations | 保留 | Graph 模块入口激活次数与模块轮数上限；parallel_frontier / prepare 规则共用。 |
| ceased | 保留 | Graph 已耗尽轮数的节点/模块出口，用于路由推进和结算证据。 |
| results | 保留 | Graph 已接受节点结果、不可变 CommitRef、Plugin 观察、中断事实及 Graph sequence。 |
| decided | 保留 | Graph 边选择与 source invocation/result sequence，数据依赖和反馈环使用。 |
| sequence | 保留 | Graph 逻辑时钟：commit_node 对 result 与 outgoing 决定各递增一次；logic 关闭失效边、模块拒绝也递增。一次 Kernel 提交可产生多个 Graph 序号，Human controls/拒绝提交不改变它；某些递增不留下新结果或边，因此不能用当前映射的 max 或 Kernel version 代替。已有冻结回放验证其精确语义。 |
| error | 删除 | Kernel Outcome::Failed.detail 是失败原因。HTTP 保留 state.error（失败时取同一次 projection.reason，其他相位为空字符串），Timeline 继续显示；不新增第二份持久失败事实。 |

RunRecordFormat 和 GraphRunRecordWire 保留在 Graph schema 边界：前者使内存中不能构造不支持格式；后者保留 serde duplicate/unknown-field 拒绝，并将旧字段映射为 C6 具名错误。Resource.version 表示接受的状态变更次数，不是 schema 版本。

## CompletionFact

结论：四个指定变体**无可删**，均为当前执行端口协议，非旧 Runner 专用。

| 变体 | 当前生产使用证据 | 保留理由 |
| --- | --- | --- |
| NotStarted | node_host/facts.rs:52 的缺完成事实；goose_acp.rs:386 缺原生调用事实；scheduler/execution.rs:39 | 尚未执行的 invocation 正常派发，身份和授权仍由宿主校验。 |
| Resumable | goose_acp.rs:398 的 native/version 2 未完成事实，:416 接续原生会话；scheduler/execution.rs:39 | 当前 Goose 原生 Session/checkpoint 接续，不能删为自动重执行；未知效果先核查约束保留。 |
| Uncertain | node_host/facts.rs:46 的已开始无完成事实；node_host/execution.rs:197–207 的 host/Goose 事实冲突；scheduler/execution.rs:36 | 转为 Code + Observation stop，未知结果绝不冒充成功或自动重放。 |
| Yielded | node_host/assistant.rs:99–145 的 yield_assistant 和 interruption 事实加载；scheduler/execution.rs:62 | 当前助手打断、控制 Artifact、普通路由与 suppressed/投递语义。 |

Completed/Failed 同样是当前端口的持久完成/已知失败事实，未改动。
