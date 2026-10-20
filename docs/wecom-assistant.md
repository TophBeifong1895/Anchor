# 接入 Anchor 企业微信助手

当前链路是原生 `anchor-wecom-gateway` → Rust Host 的 Session/Turn → 普通助手 Graph → Goose ACP 与授权 MCP 工具 → 网关投递。业务消息不经过 Pilot；Gateway 负责平台传输与投递账本，Host 负责用户隔离、Run、取消和会话事实。运行环境为 Linux，客户端可在其他电脑或手机上使用。

本文说明当前契约与配置方法。本机已部署群共享会话及可编辑的业务图，网关已认证；隔离真实模型验证了跨成员上下文、联网升级和旧原生历史接续。未代发真实企微测试消息，群聊与媒体的实际客户端收发仍待用户验收；证据见 [本轮验收](../.local/wecom-network-group-20261015/README.md)。

2026-10-20 本机常驻实例因 `database is locked` 退出：等待输入的空闲查询和已领取输入的重读误用 SQLite 写事务，放大跨连接写锁竞争。现改为只读事务；确有待领取输入时先结束读事务，再取得写事务，重新检查 current binding、领取 key 和最新输入。未延长 busy timeout，也未自动重放失败输入；当时实际持锁者未确认。Session 194 项、兼容 Host 常驻助手 23 项及固定 Goose 重启定向 3 项通过；首次恢复保留了旧事实，见 [锁竞争修复](../.local/assistant-lock-20261020/README.md)。

当前本机部署已按用户授权切换至最新已提交 master `a98757a` 的 Kernel Host，而非兼容旧执行器；未夹带 M4 未提交修改。不做备份，清除了旧 Run、Session/Turn、原生对话历史、Artifact 和执行工作区；模型/运行时设置、插件凭据、10 张 Graph、Library、计划及通道配置均保留，网关原传输去重/投递账本也保留以防重复外发。旧会话不再恢复，下一条可信消息创建新实例。8077/5173 同用固定生产 Web 构建；实际 HTTP Kernel 图执行及文件 Artifact 通过并已清理测试对象，浏览器登录和五个主页面通过，无页面异常或新增模型请求。该提交的 Host 常驻助手 23 项、Kernel 定向 11 项通过；未新增真实模型/企微消息验收，不代替 M4 独立全量门槛。部署身份、保留/清理范围与验证见 [Kernel 开发部署](../.local/kernel-deploy-20261020/README.md)。

## 机器人和成员身份

在企业微信智能机器人管理入口选择 API 模式、长连接，取得 Bot ID 和 Secret。默认平台端点为 `wss://openws.work.weixin.qq.com`；群 Webhook 地址不能替代这组凭证。无需向公网开放本机 HTTP 回调，网关与 Host 通过本机端点交互。

使用实际私聊事件中的 `body.from.userid` 配置成员名单。它不是显示姓名、手机号或个人微信号，也不能未经核对当成自建应用通讯录账号。接入不会读取或代答本人已有的个人私聊。

## 渠道身份与历史兼容

渠道账号使用独立于配置入口的稳定身份。部署环境与 WebUI 对同一 Bot ID 使用同一 `wecom:SHA256` 标识；Host 在接纳和恢复前将已验证的旧原始 ID 注册为该身份的别名。映射由 SessionStore 按 owner/source 原子保存，已登记的别名不得改指另一个账号，不推断缺省账号与有值账号等价。

新消息先解析账号再定位 Session，串行锁使用解析得到的 Session ID。同一事件在等价历史 Session 已接纳时不会重新执行。只有旧 Session 时复用其历史；已经存在新旧两套时选择规范身份对应的 Session，旧历史和 binding 原样保留，但禁止旧副本自动恢复或手动 resume，并在消息会话显示阻止原因。多个旧别名各有历史而没有唯一规范 Session 时返回冲突，不任意选择、拼接历史或结算未知投递。

## WebUI 配置

安装带有 WeCom 通道声明的插件及助手 Graph 后，在图编排中打开该工作流的「消息触发」，填写回复节点和插件，以及接收与主动发送的成员名单。每个 Host 只有一个企业微信入口，绑定当前工作流会替换原绑定。机器人凭据可在此保存，也可在同一插件详情中轮换或清除；两处使用同一份凭据。确认配置后启用监听，对话框显示 Supervisor 的实际状态，已启用不等于已经连接。各对话的实例在运行看板的「消息会话」中停止、恢复、退役或清理。

下方会话列表提供当前实例的停止、恢复、退役和清理。停止先等待执行者退出；退役后，下一条可信消息才创建新实例。清理会复核当前实例、未结算投递和待删除 Run，期间出现新消息会阻止清理。部署环境明确提供的字段仍由部署者管理，页面不覆盖。

## 部署配置

通过服务管理器提供配置环境；不要把凭证写入 Graph、Plugin 包或聊天。若使用部署的 EnvironmentFile，将其权限限制为服务用户可读；二进制不自动加载仓库 `.env`。

```dotenv
WECOM_BOT_ID=后台提供的机器人ID
WECOM_BOT_SECRET=后台提供的机器人Secret
ANCHOR_WECOM_USERS=已核实的成员userid
ANCHOR_WECOM_SEND_USERS=
ANCHOR_WECOM_GRAPH=wecom-assistant
ANCHOR_WECOM_REPLY_NODE=assistant
ANCHOR_API_KEYS=["至少32字节、随机生成的密钥"]
ANCHOR_API_KEY=与服务白名单匹配的密钥
```

多位成员用英文逗号分隔；空名单拒绝所有成员，`*` 表示明确允许全部平台成员。主动发送名单为空时继承入口名单。`ANCHOR_API_KEYS` 也保护管理 API。修改启动配置后按部署流程重启，保持一个 Bot 只有一个网关。

按 [Rust 部署指南](rust-production-deployment.md) 安装 Host、固定版本 Goose、`anchor-wecom-gateway` 和 `anchor-wecom-tools`。通过现有 Library/Graph API 安装 [助手示例](../examples/graphs/wecom-assistant.json) 及原生 WeCom Plugin，确认回复节点挂载 `wecom`。独立应用 API 包不包含通道声明，不能替代完整机器人部署。

常驻循环须显式安装为独立 Graph [wecom-persistent-assistant](../examples/graphs/wecom-persistent-assistant.json)，并选择 `ANCHOR_WECOM_GRAPH=wecom-persistent-assistant`、`ANCHOR_WECOM_REPLY_NODE=assistant`；不要原地替换已有 `wecom-assistant` 或改写旧 Run。这里的配置结果节点是 Agent `assistant`，投递由图内 `reply` Op 承担。样例可编译不表示手动/standalone 获得 Session host operation 授权；可信渠道接纳才绑定获准的 Session 和节点范围。生产切换仍需主轨独立确认，不因安装样例自动发生。

常驻助手和一次性助手的 Host 等待路径都不设置默认模型耗时上限；网关到 Host 的 HTTP 请求默认也不设置超时。只有明确配置 `ANCHOR_CHANNEL_WEBHOOK_TIMEOUT_MS` 时才启用外部请求超时；超时只表示投递观察窗口结束，不代表 Run 或 Goose 执行已停止。未知投递必须按账本核查，不能盲目重放。

Host 使用 `ANCHOR_RUNNER_BUNDLE_ROOT`、`ANCHOR_RUNNER_CATALOG_ROOT`、`ANCHOR_RUNNER_LIBRARY_ROOT`、`ANCHOR_RUNNER_STATE_ROOT` 和 `ANCHOR_RUNNER_WORKSPACE_ROOT` 区分资源、持久事实与工作区。部署配置完成后启动 `anchor-runner-host serve`；监听地址由 `ANCHOR_RUNNER_LISTEN` 配置。不要同时启动另一个共享状态根的服务。

Host 的 ChannelSupervisor 按渠道选择的 Graph 查找 Plugin 声明的原生 Gateway，设置本机回调地址、私有控制 socket 和 descriptor；不要求该 Graph 同时作为部署入口。正常部署不手动启动第二个网关。Gateway 的独立配置和协议见 [原生 Gateway README](../rust/anchor-wecom-gateway/README.md)。

## 对话与投递

处理较慢时，网关在同一条流式消息中显示“⏳ 正在准备”或当前工具步骤，仅在实际显示内容变化时更新（最多每秒一次、每轮 15 次），最终由回答替换。不再周期重发省略号动画，相同状态会去重。加载蓝圈及其位置由企微客户端控制，接口不提供位置参数；不能提前结束流式消息来隐藏蓝圈，否则最终回答无法更新该消息。

常驻助手是普通三节点循环：`wait_input (Op.host session.wait_input) → assistant (Agent) → reply (Op.host session.reply) → wait_input`，不在 Kernel 中加入 Session/WeCom 依赖或专用 Runner。首条可信消息惰性创建该 Session 的 current assistant Run；后续每条输入仍有独立 Turn，但复用 Run、cursor、同节点稳定 workspace 与逐节点 Goose 原生历史。等待节点把本轮输入作为不可变产物交给 Agent，不覆写 `Run.input`，空闲等待不调用模型。Session 保存 Turn/幂等、current 与历史 binding、retire 事实；Run 拥有执行进度，逐轮 invocation/fs2 Artifact 不可变，不因工作目录复用而合并。

等待输入时，无待领输入和同一 key 的幂等重读仅需一致的只读事务，不争用 SQLite 写锁。观察到待领输入不代表已获得领取权：必须结束读事务后获取写事务，再核验当前绑定、同一 key 是否已被领取及此刻最新的候选输入；并发替换、完成、领取或交接均以写事务中的事实为准，不把旧读快照升级成写事务。

私聊按成员隔离；群内 @ 机器人时，同群不同成员共享 Session、current Run 和对话上下文，私聊、不同群及不同机器人账号彼此隔离。每条输入及中断输入保留真实发言者，允许名单仍检查该成员。群必须有平台 `chatid`，回复沿原回调 `req_id` 返回；不新增主动群发能力。

同一 Session 串行，新消息打断旧轮，Host 等 Goose 与工具执行者真正退出，保存未提交文件 checkpoint 与 `Yielded` 事实，再由普通 Runner 提交 `Interruption` 控制 Artifact，沿 `assistant → reply → wait_input` 接续。旧回复标为 `suppressed`，不发送、不假装旧 Agent/Turn 成功，也不把整个助手 stop 后静默复活。已发生的外部操作不回滚，未知结果须先核查现场，不能自动重放。新实例仅在首个 Agent invocation 从可信获准的 previous 文件快照初始化一次，之后直接维护本节点 `/workspace`；跨图或跨会话历史不自动迁移。

入站已保存但 dispatch 拒绝或等待失败时，Host 在 Session 事务中只将仍 Running、尚无 Run 关联且未被新消息取代的 Turn 结算为 Failed。已经领取的 Turn 由其执行和投递事实继续结算。已停止助手返回 `409` 与具名 `assistant_stopped`，网关显示“助手已停止。请在 Anchor 运行看板的「消息会话」中恢复该助手后再发消息。”；拒绝不会留下永久 Running 的孤立 Turn，也不自动重放旧输入。

沿用既有一次性 `wecom-assistant` 时，每条输入仍对应独立 Run：新消息取消旧 Run，待旧执行退出后接续原生 Goose 会话；上轮工作文件通过授权的只读 `/previous` 提供。被取消、且模型尚未看到的用户消息仍作为 `input.interrupted_messages` 带入新 Run（最多 8 条、每条截断到 500 字符；纯附件消息只提示文件数量），这一旧路径不挂载被打断轮的附件，不能与常驻助手的 Turn 快照授权混淆。

Goose 负责 Agent loop、Provider、原生历史和 compaction；Anchor 保存 Session/Turn、Graph/Run、产物和权限事实。原生会话文件与 Host 状态应按部署备份契约一起保存，不能把单个 JSONL 当作完整会话。

网关持久保存投递事实，相关平台 ACK 才算确认；超时、断线或重启后的未知结果不会自动重发。Host settlement 重试只同步账本，不再次发送平台消息。Graph 完成、调用已接纳、平台确认与收件人已读是不同状态。

常驻 `reply` 只读本 Turn 绑定的确切 Agent commit 与 `reply_for`，不等待整个 Run Completed。`wecom_attach_image` 的登记按 Turn 保存，下一轮没有重新登记就不复用旧图片；同 Run 多轮不绕过既有发送 claim/ACK 账本。

### 停止、恢复与退役

- stop/resume 作用于当前实例，保留 current binding；收到停止请求不等于执行者已退出。
- **Host 进程启动时默认自动恢复符合当前部署的实例**。先加载渠道配置及账号映射，再检查未 retired 的 current binding、逻辑会话唯一性、owner、账号、Graph、结果节点与启用状态；符合条件且本进程没有活执行时才恢复。禁用或更换渠道不会删掉历史，也不会使旧绑定继续获得自动恢复资格；重新启用同一绑定后可继续恢复。用户显式 retire 的实例保持关闭，下一条可信消息才接纳新实例。开关 `ANCHOR_ASSISTANT_AUTO_RESUME=0`（也接受 `false`/`off`/`no`）关闭默认行为，由 Host 进程在启动时读取。
- 自动恢复复用与手动恢复相同的原子交接：先把 current 绑定退休、把新 Run 置为 current，并把「已领取但未提交」的在途 Turn 改判给新 Run、换成新 Run 的 wait key；再把仍停在 `running` 等非终态的陈旧 Run 对账为 `stopped`（此时它已不是 current，不会顺带把刚交接的 Turn 结算掉）；最后以 `previous_run = 旧 Run` 接纳新 Run。顺序保证交接已提交而接纳未执行时，重试仍接纳同一个目标 Run id，不产生第二个 current 实例。
- 自动路径写入独立会话事件 `channel.assistant_auto_resume{from,to}`；底层交接本身仍写 `channel.assistant_handover`，所以一次自动恢复会同时留下「发生了交接」和「由重启自动触发」两条事实。若接纳未完成（例如目标 Run 只有不可变 metadata 而没有 Run 记录），恢复**不移动任何绑定**，只按有界次数与退避重试并如实记录错误，不制造每启动一次就换一个目标 id 的链。
- 新 Run 的首个 work 节点准备在其 `.prepare.lock` 内一次性继承旧 Run 的稳定现场：只复制 Agent 文件本身，不带锁、owner fact、`previous-inputs/` 或旧 Artifact；失败不留半成品。来源由同一会话的可信交接链确认，连续重启时跳过尚未准备现场的中间实例。只改变插件/联网的 Graph 版本允许精确核验后交接，结构或指令改变不继承；旧现场与旧 Artifact 字节不变。
- 进程重启后若不自动恢复（开关关闭）或尚未恢复，须通过现有 Run 控制入口显式继续，并核查保存的 cursor、Goose 历史和现场，不自动重放未知效果。
- `GET /channel-sessions/{session}/assistant` 返回 `{ "assistant": ..., "needs_recovery": bool, "recovery_blocked": string | null }`；`assistant` 含 `session_id`、`run_id`、`wait_node`、`work_node`、`reply_node`，无 current 时为 `null`。`needs_recovery` 是派生值而非存储字段：绑定符合恢复资格且其 Run 在当前进程没有活执行（含 Run 记录缺失）时为 `true`。旧重复会话、已禁用或已变更的绑定返回阻止原因，页面保留历史并说明不能恢复，不显示为可恢复实例。请求遵循管理 API 的鉴权与 owner 范围，不能读取别人的实例。
- `POST /channel-sessions/{session}/assistant/retire` 接收 `{ "run_id": "当前实例ID" }`。先显式 stop 并等真实 `Stopped`，或确认 Run 已 `Completed`/`Failed`/`Aborted`；宿主 active 执行者、Session 的 Running Turn 和 pending/sending/unknown delivery 均阻断退役，冲突返回 409，不能用 retire 隐式取消/结算未知投递。
- explicit retire 保留旧 Run/历史 binding 与退役事实、解除 current，幂等重试不创建实例；retired Run 禁止 resume 或重新绑定。下一条可信消息才惰性接纳新实例。合法保存新版 Graph 不热更新旧 Run 的 Graph 快照或准入授权；新版通过退役后的新实例生效。
- 新交接必须由仍为 current 的来源发起；显式退役后不能通过 handover 重新激活。已提交交接只有同一 Session、精确 from/to 的重试才幂等成功，不允许其他历史来源冒认。配置资格检查只决定新接纳与恢复，不撤销当前 Turn 已冻结的权限或擅自重放外部效果。
- 退休实例和历史轮次可以经 `DELETE /runs/{run}` 单独删除，但 Host 按血缘判定，不按“看起来没在用”判定：不能删 Session 仍指向的 current assistant binding（先 retire），不能删投递未结算（pending/sending/unknown）的渠道轮次，且每个更新的 Run 要么已经不可能再执行（终态，或已有自己的后继因而被拒绝 resume/recover），要么已经完成会读取它的首轮执行——继承稳定现场、只读 `/previous` 挂载和 Goose 原生前驱身份都发生在更新的那一轮的第一个 invocation 里。删除顺序不限：删除前宿主先落不可变墓碑 `state/run-deletions/<run>.json`（记 Run、Graph bundle 身份、Session、回复节点与被删 Run 自己当时的上一个 Run），血缘遍历遇到缺失前驱时用同一会话的墓碑继续往后走，所以删中间某一环不会孤立更早的历史，幸存血缘仍是一条链、只有一个头；若幸存环节都已是被删环节的前驱，则该会话没有活动链，下一条消息重新开链。没有墓碑的缺失前驱仍然 fail-closed。删过的 Run 不能恢复，墓碑不随删除移除。要一次清掉整条会话历史可以删除整张 Graph，但会连 Graph bundle 一起删除。

### 插件/图更新的操作顺序

在工作流页面修改助手节点的插件选择或联网设置并保存，各会话从下一条消息起应用，保留同一 Run、Goose 历史和工作文件；无需再次点击运行。当前 Turn 及其恢复继续使用已冻结的权限，移除插件不会中断已开始的调用。Graph bundle 只引用 Plugin id，保存不改写 Library。

Pilot 的插件开关归单个 Session 所有；工作流页面的选择归 Graph 节点所有，影响使用该图的相关会话。两者共用 Library、挂载解析、联网规则和 MCP 执行，保存入口按各自事实归属处理。用户在工作流页面保存时，授权当前已安装的 Plugin 声明；即使曾使用同一组 Plugin，声明升级后也可重新授权，从下一 Turn 使用。已冻结 Turn 的权限与恢复保持原值，Agent 保存草稿不获得这项授权。

消息会话的插件状态比较当前 Graph 与助手已冻结的配置，不表示模型、MCP 服务或凭据连接测试通过。状态查询使用与 Graph 写入同一标识的共享读租约，多个会话或标签页的查询互不排斥；Graph 保存、准入与授权写入仍持独占租约，写入期间读取可以暂时不可用，不能因此重建助手或重放工具调用。`unavailable` 的具体原因可在消息会话行的「详情 → 插件配置」核查；会话 UUID 是会话标识，不是失败的插件 ID。

修改指令、节点或连线等结构时，仍有 current 助手的保存请求返回 409：先停止并等待执行及投递收束，再退役现有实例，然后保存新版；下一条可信消息创建新实例。普通 Graph 仍保留准入快照。插件准备失败明确报错，不悄悄恢复已移除的权限。

企业微信业务图只需由 `ANCHOR_WECOM_GRAPH` 选择，不应同时设成 `ANCHOR_RUNNER_GRAPH_NAME`/`ANCHOR_RUNNER_BUNDLE_ROOT` 的部署管理入口；后者有意禁止网页改写。联网还须在助手节点允许 `network:true` 并挂载 `network` Plugin，且会话未禁止联网。勾选后需保存，新的插件与联网选择从下一 Turn 生效；会话明确禁止联网仍优先。

Plugin 代码通过既有 Library 安装流程更新，保留声明的可变数据；后续节点准备读取当前代码，并继续校验 Run 的准入权限。升级不要求 abandon 活动 Run，也不扩大权限。独立运行的企微 Gateway 不因文件更新自动换进程，需要由已有 supervisor 重启后加载新程序；凭据和投递账本继续使用原位置。

`POST /runs/{run}/abandon` 用于主动结束一个不再允许续跑的任务，使其成为 `Aborted` 并保留审计事实；它不是 Graph/Plugin 升级的前置条件。

Graph cascade 删除须先保护仍 active/未收束执行、共享历史与未结算投递，再 retire 目标助手，按 owned Run/Turn/invocation bindings 清理 input/reply/yield/checkpoints、Artifact 与工作区；不能按整个根目录扫除，也不能把停止/退役等同于删除。

## 业务工具与附件边界

显式挂载 Plugin 才能查询或操作业务系统。需要联网的 MCP 要求节点 `network=true`，仍受宿主授权；Plugin 更新不能扩大已接纳 Run 的权限。Bot ID/Secret 与自建应用的 Corp ID/Agent ID/Secret 是两套凭证。

- `wecom_send_message(userid, content)` 通过受限宿主控制通道主动发送 Markdown。仅在用户明确要求或已经授权的定时任务中调用，目标须在允许名单内；普通回复不重复调用发送工具。
- `wecom_attach_image(path)` 是受限回复节点工具，选择工作区或授权只读输入中的真实 PNG/JPEG。登记图片不等于平台成功投递：Host 在交给网关前会重新校验数量、单张/合计大小、PNG/JPEG 与 md5，网关再用三阶段上传换 `media_id` 并以原始回调帧单独发出一条 image 消息，投递结果记录在网关 `deliveries`（`image:<inbound>:<index>`）中，只有确认后才算平台接收。出站任意文件类型平台未证实，本轮不支持。
- 自建应用 MCP 由 `anchor-wecom-tools` 提供 `wecom_send_text`、`wecom_send_markdown` 和 `wecom_get_user`，另行配置 `WECOM_CORP_ID`、`WECOM_AGENT_ID`、`WECOM_SECRET`。未配置的可选 MCP 不影响基础机器人聊天。

Rust Host 支持可信入口提交规范化 Base64 附件，冻结 SHA-256、大小与 MIME，并以 `/in/channel` 只读挂载；一次性助手按 Run，常驻助手按可信当前 Turn 保存和选择，不把 Run 的所有历史附件开放给模型。入口拒绝宿主路径、非法文件名、伪造类型和越界数据；Goose/provider 是否能理解图片需单独验证。

常驻等待 invocation 还固定一份有界 `interrupted_messages` 快照：只选当前输入之前、最近一次 confirmed 投递之后最多最近 8 条中断输入，附文本及真实附件引用，分别只读挂载 `/in/channel-pending/<turn>`。模型提供的路径不能扩大范围；后续轮次越过 confirmed 边界后不再自动挂载这批附件。原生 Goose 历史可以保留过去的消息/图片上下文，但不等于 Sandbox 获得全历史文件授权。

Host 首次 wait binding 查询最近 9 条来检测截断，只授权最近 8 条并保存 `pending_truncated`；`output.interrupted_messages_truncated=true` 时，Agent 必须说明更早中断输入未自动纳入/未完整阅读，不能声称信息全保留。这个标记不扩大有界 scope，不改变数据库 schema；旧 facts 默认 `false`。

官方 image/file/voice 回调由原生 Gateway 自己接入：回调只带短时下载 URL 与 AES key，网关在自身进程内完成 HTTPS 下载与 AES-256-CBC 解密（限额 16 项、20 MiB/项、50 MiB/事件），只把解密后的 `{name,data_base64,media_type}` 交给 Host 的既有契约，临时 URL 与 key 不进入 Host 或 ledger；ledger 只保存描述符，所以重启恢复时会重新下载，URL 过期就如实失败而不是伪造成功。含媒体的 mixed 消息支持文本与媒体混排。boundary 仍然明确：不支持 video/audio 回调；Host 不做旧文本提取、Office/PDF/OCR，文档内容以 `/in/channel` 中的真实字节可读为前提，能否理解由模型与授权工具决定；不通过模型提示词绕过这些边界。

成员名单控制助手入口；业务 MCP 使用操作员配置的凭证。对话隔离不能代替下游系统的逐用户数据授权。审批、借款与报销等 API 仍需按实际业务模板接入。

## Docmost 和验收

可在助手节点显式挂载 `docmost` 并开启授权网络，以服务环境中的 `DOCMOST_API_KEY` 访问知识库；原生附件上传工具见 [Docmost README](../rust/anchor-docmost-tools/README.md)。搜索、读取和页面变更均以当轮实际工具及权限为准，不声称本机配置已经部署成功。

常规运行验证使用 [确定性 Goose fixtures](runtime-contract-tests.md)，经过实际 Host、共享 Runner、Goose ACP 与授权 MCP，替换模型传输。Gateway 的本地传输测试为：

```sh
cargo test --manifest-path rust/Cargo.toml -p anchor-wecom-gateway --all-targets --locked
```

慢回调并发回归通过双向通知闸门建立顺序：旧回调进入等待后，控制发送和新回复必须在闸门关闭期间完成；随后释放旧回复，等待 ledger 确认旧事件完成，并核对没有额外投递。不以固定睡眠推断新旧消息顺序，也不放宽已有超时。复现、验证范围及其他用例的并发超时见[本机证据](../.local/wecom-callback-sync-20261019/README.md)；这项修正只涉及测试夹具，不改变网关产品行为。

常驻路径的定向 integration 位于 `rust/anchor-runner-host/tests/goose_persistent_assistant.rs`，需要固定真实 Goose 二进制与本机 Bubblewrap，但模型传输为确定性本地 Provider。按 fixture 文档准备隔离环境后可运行：

```sh
cargo test --manifest-path rust/Cargo.toml -p anchor-runner-host --test goose_persistent_assistant --locked -- --ignored --test-threads=1
```

2026-10-15 固定 Goose 常驻 integration 的完整 10 项已全部通过，含默认自动恢复与关闭 `ANCHOR_ASSISTANT_AUTO_RESUME` 后显式恢复；自动恢复实际核对旧 Run 对账、交接事件、同 Goose Session、新实例继承现场和旧 fs2 不变。另覆盖同 Run 两轮/稳定 workspace、等待零模型调用、打断后的 checkpoint/Yielded/Interruption 与 suppressed 回复、双用户隔离、附件只读和有界、图片按 Turn/回执隔离、退役与一次性 seed。[运行日志](../.local/reasoning-roundtrip-20261015/goose_persistent_assistant.log)列出实际 10 项及[本机场景证据](../.local/reasoning-roundtrip-20261015/native-remaining/2900051/)。领取后、产物前重启、截断提示、未知投递阻断删除与 owned 清理仍由 Session/Host 定向测试另验。

本轮隔离真实 `deepseek-flash`/Responses 两轮通过：同 Run、同产品 Session 和同 Goose Session，每轮读取当轮附件，经 `anchor_read` 的实际回执校验 `final_result`，再由 `session.reply` 生成回复；第二轮也正确回忆第一轮读取的标记。6 次真实模型请求均 HTTP 200，两个 Turn Completed，两份本地投递回执 Confirmed。[真实验收证据](../.local/reasoning-roundtrip-20261015/real-assistant/evidence.json)明确使用合成企微 HTTP 事件和本地结算，零真实企微消息，不覆盖真实公网投递、并发/打断或媒体业务。

Goose 的 Provider 错误仍可能以助手正文结束，不能仅用 Turn `completed` 或回复正文认定模型请求成功。原生 reasoning 回传已验同一服务/模型的续用；跨 Provider、模型或账号的 opaque/encrypted reasoning 兼容尚未验收。

修复已部署至本机 8077/5173，企微网关重新认证，原助手恢复至等待输入；模型配置、凭据和会话记录保留，见[上线核对](../.local/reasoning-roundtrip-20261015/deployment/after.json)与[数据核对](../.local/reasoning-roundtrip-20261015/deployment/preservation.json)。本轮没有发送真实企微测试消息。

真实账号的验收应分别检查基础收发、同群跨成员上下文、私聊/不同群隔离、补充消息取消、进程重启、允许名单和媒体。出站图片以网关 `deliveries` 的 `image:<inbound>:<index> = confirmed` 加上用户实际看到图片为准；入站附件以 Host 附件 manifest 的 name/sha256/size/media_type 与 `/in/channel` 内实际字节为准。本地 fixtures、隔离真实模型与网关认证不代替真实公网投递，未跑通的路径不写成已通过。
