# Rust Host 与 WebUI API 契约

React/TypeScript WebUI 调用 Rust Host 的资源 API。Host 可直接托管编译 Web bundle；应用服务按 Run 归属调用共享 Runner 或显式 Kernel 探针，持有执行事实，前端负责投影与控制请求。当前证据与待验收项见 [开发台账](pilot-development-plan.md)。

## Graph、Run 与 Artifact

| API | 契约 |
| --- | --- |
| `GET /graphs`、`GET /graphs/{graph}` | Graph 身份、定义与活动 Run，保留作者 JSON/layout |
| `POST /graphs` | `{name,definition?}` 创建普通 Graph |
| `PUT /graphs/{graph}` | `{definition:{...}}` 编译并保存，通过 Library 绑定资源；常驻助手插件/联网调整下一 Turn 生效，存在 current 助手时结构修改返回 409 |
| `POST /graph-validation` | `{definition:{...}}` 校验而不保存/执行；无效输入与语义诊断区分 |
| `GET /graphs/{graph}/delete-precondition` | 返回 `{graph,precondition}` 与 ETag，`no-store`；交互确认在打开时捕获版本 |
| `DELETE /graphs/{graph}` | Web/Pilot 确认使用捕获的版本；HTTP 发送 `If-Match: "{precondition}"`，资源变化或重建返回 409。省略头保留显式无条件兼容语义；活动/可继续 Run、定义或未结算调用引用仍阻止删除，只清本 Graph 数据 |
| `POST /trigger` | `{graph,input?,execution?}` 接纳为 `{run,graph}`，busy 冲突；execution 缺省或 `legacy` 使用旧路径，`kernel_probe` 显式选择串行探针，其他值/类型返回400；不接受调用方绝对路径 |
| `GET /runs`、`GET /runs/{run}` | 状态、路径/轮次、trace、节点、调用、Plugin 与触发来源 |
| `POST /runs/{run}/pause|stop|resume` | pause 是 stop 的别名，返回 operation=stop、alias=true；保留 snapshot/input/身份；R2 未验收 |
| `GET /runs/{run}/files/{node}`、`.../{path}` | 不可变 Artifact 文件、预览/下载；拒绝 host-path 越权 |
| `GET /graph-relations`、`GET /channel-sessions` | 真实定义、调用与可信来源投影 |
| `GET /timeline`、`/schedules` | Run 与计划事实；时间戳未知如实披露，不编造历史 |

`GET/PUT /graphs/{graph}` 的 `plugin_activation: {mode,graph_digest}` 表示配置生效规则：`next_turn` 为常驻助手，`new_run` 为普通 Graph，不表示外部插件已可用。`GET /channel-sessions/{session}/assistant` 另投影 `plugin_update: {state,message?}`，状态为 `current/pending/restart_required/unavailable`；未知或读取失败不能显示为已生效。Web 在保存后提示下一条消息生效，活动 Graph 打开已有运行，渠道 Graph 打开助手管理，避免重复 trigger。

HTTP DTO 是产品投影，不直接输出执行器私有记录。未知结束时间、exit status 或来源必须保持未知；下游失败不能改变上游已提交事实。文件复制/迁移可能改变基于 mtime 的历史时间精度。

本分支 R2 的 Run 列表、详情、时间线、子调用及 Pilot Run 查询共用 `projection: {phase,waiting_for?,outcome?,reason,active,actions:{stop,resume,abandon},updated}`。phase 为 executing / waiting / settled；waiting 才含 human / child / effect，settled 才含 done / failed / escalated / unknown / aborted。旧顶层 status、state.status 和 control_requested、投影 terminal/quiescent/needs_attention/pause 均退出对外 Run 契约；Session Turn、投递、监听与节点结果的状态仍属各自事实。

基础动作只由 Phase 和 worker 派生：执行中可 stop，未结算可 abandon，无 worker 的 Executing 或 Waiting(Human) 可 resume。Session/身份/当前助手/被取代会话条件在边界收窄；写入仍执行现有授权、scope、lease 和控制守卫。旧 BudgetStopped 等实际写入拒绝仍存在，不能把新读取动作当成写入已迁移。Web 直接使用服务端 actions；缺少投影时显示不可用。

`kernel_probe` 持久权威为 Kernel FileStore；内部兼容 GraphRunRecord 不回写。updated 来自 Host 的独立观察文件，无观察为 null；旧 Run 仍读取 FileRunStore mtime。时间不参与控制。stop 接受后进入等待并取消 worker，排空前不能 resume；无 worker 的 Executing 以 active=false 表示可接续。HTTP pause 和 Pilot run_pause 均调用 stop，返回 alias=true；这会即时取消当前节点，并使 Session 上原来拒绝的 pause 按 stop 处理。旧 attempt recovery decision 和 Kernel Run/所属 Graph 删除仍具名拒绝，未扩展非串行支持。

**R2 停止、未验收**：新别名与现有完成产物、恢复和 Session 仲裁断言冲突，未更改控制写入规则或弱化非状态断言。这里记录分支实现，不能作为发布验收；见 [R2 现场与测量](../experiments/constitution-host-preflight/r2/measurement.md) 与 [验证](../experiments/constitution-host-preflight/r2/verification.md)。真实 Goose/Provider 与部署未新增验收。

Run 详情另返回 `definition: {entry,nodes,edges,ops}`，仅来自该 Run 的冻结 snapshot，包含已展开的 inline 模块。它是只读展示拓扑，不含作者 layout、脚本、指令、输入及 host 参数，不能当作完整可重运行的作者定义；当前 Graph 的修改、损坏、移除或未保存草稿均不改变它。Run 文件与 Pilot 附件统一通过鉴权 Blob 下载并显示失败反馈。

## Session、Library 与渠道

Session API 包括创建/重命名/删除、消息、Turn、SSE、停止、问题与回答。Turn 按 `request_id` 去重，SSE 支持游标回放和重连；同一请求不得再次执行。问题/回答归 Session/Turn，模型历史归 Goose。

Pilot `POST /sessions/{session}/turns` 接受 `{request_id,message?,attachments?}`，文字与附件至少有一项；附件项为 `{name,data_base64,media_type?}`。同一 `request_id` 必须保持原文字与附件内容，`resume:true` 不能同时提交新文字或附件。输入限制为 16 个文件、单个 20 MiB、合计 50 MiB；其中图片只接受 PNG/JPEG/WebP，最多 8 张、单张 10 MiB、图片合计 20 MiB。只有该上传路由放宽至 72 MiB 请求体。

Turn 的 `attachments` 列出输入；消息投影在含附件的 Turn 中附加 `turn` 与 `attachments`。继承输入保留原上传 Turn 的 URL，不重复生成用户上传消息。文件引用包含 `{id,name,size,sha256,media_type,url,download_url}`，不含宿主路径或密钥。`GET /sessions/{session}/turns/{turn}/attachments/{id}` 核验既有 Session 归属后读取原件；只有校验后的支持图片可内联显示，其他文件强制下载，`?download=1` 对图片也强制下载。预览和下载均沿用 Bearer 鉴权。

Pilot 用 `publish_file` 发布工作区文件，SSE 发出 `{type:"attachment",attachment:引用}`，历史从同一事件投影输出附件。发布保存当时的不可变版本，后续编辑工作区不改变已交付文件；原生工具返回的有效图片也进入同一输出路径。自动接续和恢复引用原获准输入，不扩大到全会话附件；删除 Session 沿既有归属清理原件。Web 支持选择、拖拽、粘贴图片及附件草稿，使用鉴权请求获得 Blob 后预览或下载。

Pilot 在可开始对话、没有消息且没有待处理执行或问题的空页面显示任务入口，示例只填入并保存草稿；未选择会话时，首次发送沿现有接纳接口创建 Session/Turn。进入对话后保留同一个输入框；插件面板默认收起，选中会话后预读真实挂载状态，窗口尺寸变化保留用户的开关选择。

Plugin API 提供 catalog、安装/移除与 owner-bound 授权事务。OAuth、模型与远程 MCP 的真实兼容性需要独立验收；本地工具测试不等于外部授权流程已经通过。

`GET /plugins` 返回当前目录信息，不返回代码 `digest` 或使用说明；`GET /plugins/{id}` 按需提供完整详情、`digest` 和使用说明。HTTP、Pilot 与 `GET /sessions/{id}/plugins` 共用基础目录读取与健康观察：`definitionValid` 表示清单/声明路径结构有效，`dependenciesReady: bool|null` 表示当前依赖视图就绪；`contextAuthorized`、`connectionVerified` 没有实际证据时为 null。基础 `available` 由前两项决定，Session 可叠加上下文否决，不代表执行授权或连接成功。清单损坏项保留不可用行，Session 中缺失插件的已开启引用也保留以便撤销。启用仍校验完整定义和依赖；关闭不要求插件健康，保留归属、归档、required 和发布并发约束。列表不读取全部代码、生成 digest 或执行绑定；完整定义只在详情、挂载启用和执行时读取。

Graph、Pilot、Library、历史 Run 共用插件设置的控件、排序、筛选、加载、错误及反馈；页面只提供上下文与操作适配。Web/Responses 新 Pilot 共用创建用例设置默认 network 挂载，通用 Session 创建及既有会话不改变。新增角色与首次并行区域通过共享角色构造消费 `GET /settings/model` 的 `execution.default_reference`，不在 Web 维护另一套模型默认值。

可信渠道入口绑定会话、真实发言者、Graph、回复节点与本地冻结附件。`ChannelIdentity.conversation_kind` 可为 `single/group`，缺省 single 且序列化省略；群按 conversation 共享 Session，sender 始终表示本条消息的真实成员。WeCom 对应字段仍为 `metadata.chat_type`。同 Session 串行，私聊/不同群隔离；中断输入保留 actor，回执核对 actor、kind 与 conversation。普通 Graph 和 Pilot 管理入口分别接纳，不把助手业务交给管理 Agent。

Webhook 与 Responses 共用接纳和身份校验。Responses 只实现冻结子集，不能宣称兼容完整 API；SSE 代理需关闭缓冲。`/health` 是 liveness，`/ready` 是配置 Graph readiness，不检查模型网络。

## Web 管理入口

管理操作不要求 Pilot Session 或模型调用。HTTP 与 Pilot 复用 Host 应用用例及配置事实；旧执行配置与环境、渠道中的部署字段保持只读，新模型配置槽可独立编辑。设置、凭据与插件预检响应使用 `no-store`，密钥只通过专用表单提交，不进入聊天、日志投影或浏览器存储。

`POST /sessions` 未指定 ID 时创建 `pilot-private-` 会话，与 `responses-` 会话一样按当前 API key 归属；无认证的 loopback 使用 `local`。因此新 Web 会话与插件详情共享同一 owner 的凭据。旧显式普通 ID 仍属共享 `local`，不自动迁移或跨 owner 复用密钥；旧会话表单明确显示该作用域，完成的配置请求只代表历史操作。

| API | 契约 |
| --- | --- |
| `GET /settings/model` | 返回下一轮默认模型 `execution`、固定 `providers.default/backup` 及兼容旧配置；每槽有 `fields: {endpoint, wire_api, flash, pro}`、`api_key_present`、`configured`、`source`（`legacy/host/empty`），不返回密钥；`configured` 只表示字段齐备，连通性须单独测试 |
| `PUT /settings/model/providers/{slot}` | 仅接受 `default/backup`；保存上述 fields 与 `api_key: {action: keep/set/clear, value?}`，可保存未填全的配置，返回完整状态；下一轮按引用读取最新值，正在执行的轮次保持原快照 |
| `POST /settings/model/providers/{slot}/test` | 接受同一草稿并显式指定 `tier: flash/pro`，发送小请求；不保存、不选择默认模型、不自动尝试另一槽 |
| `PUT /settings/model`、`POST /settings/model/test` | 保留旧执行配置管理/测试兼容接口；已保存的新槽不随旧配置编辑而改变 |
| `PUT/DELETE /settings/model/credentials/{provider}` | 管理保留的服务凭据；旧配置按地址与协议标识，命名槽历史另绑定 `slot`。两槽当前配置通过各槽 PUT 管理 |
| `GET /settings/runtime`、`POST /settings/runtime/prepare`、`POST /settings/runtime/cancel` | 读取 Python/Rust 状态与初始化操作；准备显式携带 `allow_network`，复用既有准备器、部署限制和取消收束 |
| `GET /plugins/{plugin}/credentials`、`PUT/DELETE /plugins/{plugin}/credentials/{target}/{field}` | owner-bound 字段状态：缺失、已保存、声明失配、部署管理；修改核验当前授权摘要，不返回原值 |
| `POST /plugins/install-preflight`、`POST /plugins/{plugin}/uninstall-preflight` | 候选或卸载影响预检；安装接收 GitHub tree URL 或受限普通文件列表，不接受宿主任意路径 |
| `GET /plugins/preflights`、`GET/DELETE /plugins/preflights/{token}`、`POST /plugins/preflights/{token}/confirm` | owner-bound 预检、操作观察与精确确认；确认复用 Library 发布锁和应用前置条件，同一操作不重复执行 |
| `GET/PUT /channels/{platform}/configuration`、`POST /channels/{platform}/enabled` | 复用渠道管理适配器与 supervisor；启停携带预期绑定，保存/启用与实际连接分别投影 |
| `GET /channel-sessions/{session}`、`.../assistant`、`POST .../assistant/retire`、`DELETE /channel-sessions/{session}` | 校验渠道 owner，复用 current binding、静止与未结算投递限制；退役不会立刻创建新实例 |
| `POST /schedules`、`PUT/DELETE /schedules/{schedule}` | 新建、编辑、暂停/启用与删除共享计划；更新保留 ID/创建时间，输入修改保留下次时点，改规则或重新启用只安排未来执行 |

`execution` 投影下一轮的默认选择：`default_reference/source/configured/fields/api_key_present/error`；其中 `fields` 是选定模型的公开字段，顶层旧 `fields/configured` 仅保留兼容含义。Pilot 和未指定模型的节点使用 `models.default.flash`，Graph 可显式选择 `models.default.pro`、`models.backup.flash/pro`；不自动降级。default 尚未保存时兼容旧默认配置和预填，已保存但不完整时明确失败；四个设置引用以外的原显式模型别名保留原解释。旧 invocation 即使用过同名别名，恢复时仍核对其原始绑定。

新 Pilot Turn 和新节点 invocation 读取最新设置，包含同一 Run 的后续节点或常驻助手下一轮；进行中的执行保持独立快照，无需重启 Host。加载已有 Goose Session 后通过原生模型配置接口选定本轮模型，保留历史。恢复同一个节点 invocation 仍核对原模型/服务绑定，不能改用新服务；槽的公共历史和分服务凭据复用同一私有存储与锁，历史凭据投影增加 `slot`，可经现有凭据接口轮换或清除。两槽即使地址相同也使用独立凭据；换地址/协议的 `keep` 不携带原密钥。初始化状态可跨页面刷新读取，Host 重启后的未完成初始化显示中断，重试复用准备器检查现场。

模型测试保留 20 秒总时限。收到成功 HTTP 响应后读取正文超时返回固定诊断 `model provider response timed out`，网页说明已经连接、尚未得到完整模型结果；发出请求阶段超时仍为 `model connection test timed out`。HTTP 200 或保活消息不能单独作为模型测试通过的依据，也不证明服务整体不可用。

Runtime 状态的 `python_prepare_available` 表示当前配置满足 Python 准备条件：未被部署禁用、无准备字段冲突，且有完整外置解释器与准备器或有效的发行包 bootstrap。它与 `python_configured`（已有解释器）、`python_bootstrap_available`（发行包工具）分别判断，不保证后续下载成功。Web 按该字段开放按钮；旧 Host 缺字段时保守回退到 bootstrap 状态。GET 通过有界本地探测返回 `python_version`（完整实测版本）与 `python_version_error`；失败时版本为 null，不使用路径或默认安装版本推测。刷新会重新检测；prepare/cancel 响应不执行探测，Web 保留最近观察直至下一次 GET。部署管理的 Python 展示实测版本，不允许网页改版本，准备请求传 `version: null` 以复用原解释器。

插件预检依次投影 `preparing/ready/running/succeeded/failed`，确认携带具体替换与数据删除选项，重复确认观察同一操作。文件导入最多 2,048 项、解码后 32 MiB，请求上限 48 MiB；拒绝绝对路径、路径穿越、重复项及链接/特殊文件。预检和操作句柄驻留当前 Host，重启后客户端核对实际 Library 状态再决定是否重新预检，不把句柄丢失当作安装失败。独立保存凭据会完成同 owner 且仍有效的待处理请求；若保存成功但通知同步失败，响应保留成功事实并附带提示，不让用户误认为密钥未保存。

新建工作流与运行输入使用正式表单；运行输入草稿保存在当前浏览器。Graph/Run 的 Pilot 协助入口只带入对象引用到未发送草稿，不自动执行任务。插件、渠道、设置页面按各自需要读取状态，不恢复全工作台后台轮询。

## 权限、观察与验收

API key、工具范围、路径/网络、原生 Session scope 与精确删除前置条件由 Host 强制执行。提示词、Plugin 说明、前端隐藏按钮和模型输出都不是授权。

trace 展示 ACP 的原生文本/工具内容，按 `tool_call_id` 配对；媒体有独立可接受类型/解码边界。投影用于查看，恢复仍使用持久 Run/工具事实与 Goose 原生 Session。普通 assistant text 不冒充 canonical 完成摘要。

浏览器验收使用实际 Rust Host、固定 Goose 与 Node 本地 Provider，检查 Graph CRUD、真实执行、暂停/停止/继续、历史、文件、Session 和必要提问。UI fixture 只证明展示；未配置而跳过、实际端到端与真实业务验收须分别记录。运行方法见 [小图回归](runtime-contract-tests.md)。
