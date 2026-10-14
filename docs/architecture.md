# 当前架构

Anchor 的维护实现是 Rust Host + 共享 Runtime Kernel + Goose ACP，WebUI 为 React/TypeScript。官方工具、发行 builder 和开发检查位于同一个 Cargo workspace。产品目标见 [产品与系统架构](product-architecture.md)，当前验收结论和未关闭范围见 [验收摘要](pilot-development-plan.md)。

## 执行链与所有权

```text
WebUI / CLI / Webhook / 通道
        ↓
Rust Host 应用用例：接纳、鉴权、Session、控制、Library、Scheduler
        ↓
共享 GraphRunner：节点、路由、反馈、fanout/join、Run 状态
        ↓
NodeExecutionPort
  ├─ AgentNode → Goose ACP → 授权 MCP → Sandbox / Plugin
  └─ OpNode    → Bubblewrap 命令 / Graph Call / 通用 host operation
        ↓
Artifact、节点历史、工作区、Run / Session / Turn 事实
```

| 模块 | 职责与事实 |
| --- | --- |
| [anchor-runtime](../rust/anchor-runtime) | Graph 编译、IR、Runner、RunStore 和节点执行端口；不依赖 HTTP 或具体业务 |
| [anchor-runner-host](../rust/anchor-runner-host) | API/应用接纳、共享 Runner、Goose 接入、Artifact、渠道协调、调度和控制 |
| [anchor-graph-host](../rust/anchor-graph-host) | bundle 读取、资源绑定、独立 Graph Call 和恢复关联 |
| [anchor-platform-session](../rust/anchor-platform-session) | Session/Turn、幂等请求、问题/回答、事件游标、投递事实及助手 current/历史 binding 与 retire 事实 |
| [anchor-library](../rust/anchor-library) | Plugin/tool catalog、受审资源、凭据引用和 OAuth transaction |
| [anchor-sandbox-bwrap](../rust/anchor-sandbox-bwrap) | 文件挂载、命令、网络与取消边界 |
| [anchor-mcp-host](../rust/anchor-mcp-host) | MCP 连接、工具与媒体内容映射 |
| [anchor-scholarly](../rust/anchor-scholarly) | 官方学术搜索、论文读取与 MCP |
| [anchor-docmost-tools](../rust/anchor-docmost-tools)、[anchor-wecom-tools](../rust/anchor-wecom-tools) | 原生业务工具 |
| [anchor-wecom-gateway](../rust/anchor-wecom-gateway) | 企业微信 WebSocket、ACK、去重与私有控制 socket |
| [anchor-rsi](../rust/anchor-rsi) | RSI/周报采集、证据、评审门禁与安全产物装配；复用 Host 的路由与 Runner |
| [anchor-distribution](../rust/anchor-distribution) | 受审闭包的 source-free 归档，不执行输入程序 |
| [anchor-devtools](../rust/anchor-devtools) | 确定性回归、候选构建、部署预检与切换盘点 |
| [apps/web](../apps/web) | Graph 画布、Run 时间线、节点对话、Session 和产物投影 |

Goose 拥有 Agent loop、Provider、原生会话与 compaction；Anchor 拥有 Graph/Run、宿主授权、Artifact 和平台 Session/Turn。工具记录、API 与 UI 都围绕相同身份，UI 缓存不能成为第二份运行事实。

## Graph 与节点

作者 JSON 定义 `agents`、`ops`、`graphs`、`nodes`、`edges` 和 `layout`。`GraphSnapshot::from_authoring` 编译为执行 IR，保留作者定义供 Web 编辑；内联模块在执行前展开。bundle loader 绑定清单内资源并校验摘要。资源安装与 Graph 保存都经过 canonical Library 校验。

AgentNode 引用角色与节点级 `plugins`，角色只复用模型、指令和权限。Op 定义在 `run`、`call`、`fanout`、`join`、`host` 中恰好选择一种；Op 不继承 Plugin。`Op.host` 通过 `NodeExecutionPort` 交付通用 operation JSON；Kernel 不依赖 Session 存储或 WeCom 协议，具体能力由 Host 实现和授权，手动/standalone 入口默认没有 Session host operation 授权。反馈回访从同 Run 最近已提交产物延续，新 invocation 保留自己的执行现场；普通跨 Run 不自动继承。

同一 Run 由唯一协调者维护。配对 fanout/join 允许独立串行分支并发，所有成功后收束；分支失败取消同伴。嵌套、分支交叉和区域内 Graph Call 等不支持的组合在接纳前拒绝。独立 Graph Call 使用同一 Runner，`wait` 与 `detach` 的父子身份和生命周期分别持久化。

## 数据布局

Host 分别配置 bundle、catalog、state、workspace 和 Library 根。release 目录只读；可写根放在 release 外。Host 在监听前验证配置并持有 state-root `deployment-writer` 锁。开发脚本使用独立数据根，旧本地记录不会被重新接管。

```text
<release>/anchor-runtime/
├── bin/                   Host、固定 Goose 与显式工具
├── bundle/                独立 Graph 包的 Graph、manifest、Plugin ID 引用
├── plugins/network/       完整平台的原生联网引导 Plugin
├── web/                   编译后的 WebUI
└── runtime-manifest.json  文件 hash、模式与 ELF 依赖身份

<mutable roots>/
├── catalog/               可编辑 Graph bundle 与资源闭包
├── library/               Plugin、工具、授权引用
├── state/                 RunStore、Session/Turn、事件、计划、渠道与锁
└── workspaces/            节点 workspace、fs2 Artifact、只读输入与 Goose 会话
```

fs2 Artifact 的文件及谱系是节点产物权威；只读 Git 输入视图兼容需要 commit 绑定的业务检查。单节点沙箱使用 `/workspace`、只读 `/in/<node>`、`/plugins/<id>`、`/tools/<id>`；显式授权的本地输入按 Run 冻结。宿主工具目录由 Library `tool.json` 注册。共享 Python 与 Rust 的配置由 Host 统一解析：完整平台可通过 Pilot 初始化，显式部署字段仍由部署者管理。Pilot 在自己的 Session 工作区处理 Plugin 候选，不因此获得任意宿主路径或环境写权限。第三方工具的运行语言不改变核心职责。

沙箱的 `/dev` 是 bwrap 建的最小 devtmpfs，因此默认没有任何加速器设备节点。需要 GPU 的第三方工具由操作员在同一个 `tool.json` 里登记 `devices`：每一项必须是指向 `/dev` 下真实字符/块设备节点的绝对路径，Host 在装载工具时校验并把它变成该沙箱策略的精确设备授权，节点执行时按同一绝对路径 `--dev-bind`。设备既不随只读挂载根、workspace 或模型参数隐含获得，也不能登记 `/dev` 本身、目录、普通文件或 `/dev` 之外的路径；未登记的请求在创建 namespace 之前就被拒绝。设备授权是部署字段，Graph/Plugin 文本无法扩大它。

部署者可通过 `ANCHOR_NODE_COMMAND_TIMEOUT_SECONDS` 配置 AgentNode 单条 `anchor_run` 命令上限（默认 30 秒），并通过 `ANCHOR_MODEL_MAX_TOKENS` 将单次模型回复的输出预算传给 Goose；后者默认不设置，沿用 Provider 默认。节点整体墙钟控制继续生效。

插件运行依赖声明属于 `plugin.json` 的 `dependencies.python`，正式、候选及兼容旧项目的只读视图引用 Library 的共享包；新项目使用私有开发 venv。Host 使用获授权的来源与固定 uv 准备正式依赖，安装经原生确认和 Library 发布锁。包字节比较、视图 launcher 与跨解释器边界见 [依赖标准](plugin-dependencies.md#当前实现映射)；Graph bundle 只引用 Plugin ID，旧 `environment` 声明具名拒绝。

Run format 13 将准入授权与执行观察分开：`plugin_bindings.authorization_digest` 保存无凭据值的 Host 权限投影；节点结果的 `plugin_digests` 和 `plugin_views` 保存本次实际准备的代码与依赖声明摘要。代码、Skill 路径和依赖变化从后续调用读取；联网、可写路径、宿主命令、凭据来源、远程端点或 OAuth 授权声明变化不能沿用旧 Run 权限。format 12 记录只迁移格式，不填造新观察或权限；旧权限为空时，Host 与 Runner 共用完整旧 ID、代码摘要、资源与 MCP 名称核验，全部通过后由持 Run lease 的 Runner 一次性保存升级。

AgentNode 和 Pilot 的正式工具准备在同一短 Library lease 内读取一份 `PluginDefinition`，由它生成绑定、MCP 配置、挂载、依赖视图及只读披露目录；释放后才做 OAuth/MCP 网络启动。目录再与实际工具装配的 Plugin/server/remote-tool 归属关联，不解析工具名前缀猜所属资产。提示词只提供固定的发现入口，不平铺全部 Plugin/Skill 路径，不另存提示词事实。此锁保证准备期间的读取一致性，不是历史代码快照；释放之后的更新仍可替换当前代码或回收旧视图，不能据此承诺旧进程可复现。Plugin 的 `mutable` 授权独立于 Python 依赖：无 Python 包的插件也可使用声明的数据目录；`plugin.json`、`.mcp.json` 与外置 MCP 声明文件必须属于不可变代码。升级逐项替换代码、保留数据；多条目更新中途失败如实报告。依赖规则见 [依赖标准](plugin-dependencies.md)，验证范围见 [验收摘要](pilot-development-plan.md)。

AgentNode 的 Goose 接入区分工具解析已经返回的结果与外层等待被取消/超时：解析错误保留原错误类型并沿既有 Run 错误路径结算，不再次轮询已经完成的 future；只有外层中断才通知仍未结束的解析过程，并等待它回收所拥有的 MCP 子进程。

Library 审查、候选准备与导入共用代码暂存规则：选中 root 或 `.codex-plugin` 清单后只读一次，解析和暂存使用同一份原始字节；枚举在读取类型之前排除 `mutable`，不复制源数据，暂存只提供空数据位置。升级的 durability 同步只遍历代码与必要父目录，不进入数据子树。嵌套数据声明保留完整缺失尾路径，符号链接或非法父路径不能扩大写权限。

引用发布与卸载共用 Library lease。Run 在读取 Plugin 至保存持久引用之间持有此锁，执行前释放；Run 准入持 Graph lease 时不等待 catalog guard。Session 挂载与安装/卸载按 catalog guard → Library lease 排序，并在锁内复核版本和引用。卸载沿用用户批准的前置条件，不在确认后刷新；安装失败的视图清理也在锁内核查，已引用或状态不确定的内容保留。
常驻助手由 Host 显式绑定同 Run、同节点的稳定 workspace（`<workspace-root>/<run>/nodes/<node-hash>`）；Agent、Op、freeze 与清理共用该绑定。可写现场不随 invocation 新建，但每轮 invocation、完成事实与 fs2 Artifact 仍独立且不可变。工作区 owner/初始化事实与 Run 的唯一执行者共同保护接续，不能仅凭目录存在或 preparation lock 认定可接管。中断现场另存只读 checkpoint，不能冒充成功产物。新实例仅在首个 Agent invocation 从可信、获准的 previous 提交或中断快照初始化文件一次，不每轮复制 `/previous`；普通一次性 Graph 的工作区默认行为不变。

## Goose 与恢复

Goose 固定为 v1.53.0；Anchor 用上游同一份源码的 lean ACP-only 入口构建静态 x86_64 musl 二进制（recipe 见 [构建脚本](../scripts/build-goose-acp.sh)），SHA256 为 `71e76c412597b2ecd96ed20d0706e7666f31c018216e7cb5d65c5ca5c44824a7`。它与官方完整 CLI 共用 `goose::acp::server::run`，省去完整 CLI 和未使用的发行入口；Goose 库的原生平台扩展仍须由 Host 明确管理。AgentNode 与 Pilot 共用 ACP/MCP 接入。Host 绑定二进制、模型/endpoint、原生 Session 和 invocation，恢复时不静默更换模型或新建替代历史；更换二进制即改变恢复身份，未收束的 invocation fail-closed，不会被静默接管。

固定版 Goose 的 `enabledExtensions` 在新建时追加到默认扩展，加载时保留已存扩展，不能单独用作授权集合。共享 ACP 接入在新建/加载后、首次模型提示前，通过官方 `session/extensions/list`、`remove`、`add` 接口校准为 Anchor MCP 与部署显式允许的原生扩展；默认只保留提供边界指令的 `tom`，不启用 `developer` 的独立文件和 shell 工具。撤销旧扩展保留原生 Session 与历史，不直接修改其数据库；校准失败拒绝本次执行。

Pilot 与 AgentNode 共用从当前授权和 Library 派生的 Plugin/工具目录，通过有界摘要和固定发现入口按需读取。完整分页、读取、调用和兼容开关见 [Plugin 披露契约](plugins.md#发现与读取)。目录是可重建投影，Goose 原生系统提示和 compaction 承载上下文；MCP 仍按准备流程连接，按需连接尚未实施。

AgentNode 通过授权完成工具提交 `summary` 和可选 `route`；普通回答不能完成节点。Host 校验路由、核查回执与必要产物后提交 Run 事实。完成回执来自最近一次授权非完成工具的返回结果，发现与只读工具也会刷新回执；回执不匹配的诊断明确要求复制实际返回值，可通过只读核查取得新回执，不应为此重复业务操作。业务工具效果发生但返回结果缺失时，保存观察事实；继续原生会话后，Agent 先核查 workspace、Artifact 和可用业务状态，再决定下一步。工具不会自动重放，外部副作用不承诺 exactly-once。

Run 的暂停/停止请求与节点已收束是不同事实。恢复保留 Graph cursor、invocation、原生 Session 和中断记录；重启不会自动执行未结算业务或确认。旧执行器记录保留旧身份，可识别和拒绝冒充 Goose 游标；清理源码不迁移旧数据。

Host 对外 trace 投影 ACP 消息、工具观察和媒体，支持增量读取与重开节点。该投影用于观察，不代替 Goose 原生历史。真实模型、vision 与长会话效果必须分别验收。

## Session、渠道与入口

Pilot Session 保存业务关联、Turn、问题/回答和 UI 事件；Goose 保存模型历史。Turn 提交按 `request_id` 幂等，SSE 按事件游标回放。原生 MCP elicitation 经过 ACP；必要删除确认绑定确切 Graph 与资源前置条件，重启中断未完成的问题，不重放旧确认。

Pilot 与渠道附件复用 Host 的字节校验、限额、hash 和不可变保存原语，各自保留入口身份与归属。Pilot 原件位于已登记的 Session 资源 scope，Turn 只保存输入身份和文件引用；输入只读挂载 `/in/pilot`，图片通过原生 ACP image 内容交给 Goose。接续/恢复继承确切来源引用；新用户请求不自动获得历史附件。`publish_file` 固定工作区文件的交付版本，原生工具图片复用同一发布路径；输出事件驱动实时界面与历史，鉴权下载核查 Session/Turn/原件身份，删除沿既有 Session 清理。API 与限额见 [API 契约](rust-frontend-api-contract.md#sessionlibrary-与渠道)。

`GET /sessions/{id}/messages` 按既有 Turn 和 UI 事件顺序投影用户输入、回复、提问与回答；正文保留 `role`/`text`，提问与回答分别附加 `question`/`question_answer` 的归属及保存状态。回答显示字段标题和实际填写的值，拒绝与取消保持明确的用户决定，中断提问没有虚构回答。Web 将这些记录直接显示在对应的对话位置；运行中的问题仍由当前 Turn 显示一个交互表单，完成后从消息投影读取，不另设历史提问区、不重复保存消息或重放回答。

完整平台 Pilot 使用内置管理指令，不需要业务 Graph，空 catalog 可以启动和就绪。独立 Graph 宿主及显式配置的旧助手 Graph 仍保留相应准入检查；只有配置了助手 Graph 时，才检查其指令 Plugin 的自修改保护。安装和卸载的原生确认、摘要与引用前置条件不因没有业务 Graph 而改变。

Pilot 的 Graph validate/create/update 工具共用作者格式说明与最小合法示例，空 catalog 不依赖已有 Graph 猜测格式。校验直接复用 Kernel 的 `GraphSnapshot::from_authoring`；输入格式错误返回具体位置和原因，Host 能力及私有 Plugin catalog 的错误保持安全泛化。说明和示例不定义另一套编译规则，也不授予执行权限。

Pilot 的任务联网由 `network` Plugin 提供启用入口。Host 在网页与 Responses API 新建 Pilot 会话时默认保存主动挂载 `network`；已有会话不补默认，关闭设置在续聊和重启后保留，渠道和业务 Graph 会话不增加此默认。插件的 `network:true` 或远程 MCP 声明产生联网需求，Host 从当前挂载集合推导依赖启用；Session 的 `network_allowed` 是用户授权上限，依赖不能覆盖明确禁止，带 Session 的 AgentNode 仍受节点网络权限上限约束。授权与挂载共同冻结到 Turn 快照，stdio 沙箱、HTTP MCP 与 OAuth 使用同一结果；离线工具保留无网运行。Provider 的控制通道连接独立。具体状态、旧快照兼容、独立 Graph 边界和候选包契约见 [任务联网能力](network-plugin.md)。

Pilot 的普通 Turn 与 Responses 共用接纳、Session 工作区及现有文件/命令工具。Host 的 `RuntimeConfiguration` 统一解析基础环境，Session 持有私有开发现场，Library 持有候选引用和正式 Plugin；显式部署字段仍由部署者管理。准备、命令、测试、正式交付及清理的具体行为见 [Pilot 开发契约](pilot-development.md)。

Pilot 接纳保存网络和工具授权快照，执行启动时读取一次 `RuntimeSnapshot` 并冻结基础解释器和依赖准备器，包括“尚未配置”的状态；当前执行不会因其他请求更新配置而获得新能力。运行时探测和 MCP 启动不持全局接纳 gate。对话内联网授权、挂载、环境准备或凭据配置保存事实后请求幂等 continuation，当前 Turn 收束后通过同一 `PilotService::admit` 接纳下一 Turn，Web 跟随新 Turn。停止、删除、归属变化或未收束执行不能自动接续；重新接纳核查已保存请求，不重放准备或业务工具。Graph 后续节点从 Host 当前配置及当前 Library 装配，保持既有热替换语义。

网页挂载在 Pilot gate → catalog guard → Library lease 顺序内复核 Session、保存挂载并登记接续。API 保留 `plugin`/`plugins`，增加 `effect`（`next_turn`/`after_current_turn`/`unchanged`，活动源附 `source_turn`）及既有 continuation 投影。仅当前登记执行且仍为最新 Running Pilot Turn 的真实变化请求 `continuation:<source>`；多次切换合并为一次，当前 Turn 保持旧冻结快照，空闲切换不调用模型。仅正常结算后自动接纳；失败、中断、停止、归档或新用户 Turn 取代来源都不唤醒旧任务。自动接续沿原生历史只处理最新用户请求中未完成且已授权的部分，能力变化不构成新的业务任务，也不要求重做已完成结果。Web 跟随宿主来源与后继，重开时处理列表/详情之间已接纳的交接，等待接续仍可停止或发送新消息。

网页网络许可按钮只保存 `network_allowed`，影响后续 Turn；该 API 不返回挂载 `effect`，也不登记自动接续。对话内工具发起的联网授权仍按其既有 continuation 路径处理。

停止与结算/接纳共用 Pilot gate：执行已返回成功但尚未结算时，取消仍结算为 Stopped；来源已经 Completed 时保留该历史，通过幂等 `pilot.continuation.cancelled` 事件取消该来源的后续推进，即使尚无 requested 事件也适用。取消在重启后仍有效；OAuth 回调在交换 token 前和持 gate 发布授权前都核查该事实，迟到回调不能交换/保存 token 或恢复任务。该事件是 Session 控制事实，不是新审批流程或 Agent 恢复日志。

允许联网的沙箱通过独立网络 namespace 与 `anchor-sandbox-network`/slirp4netns 出网，禁用 host-loopback；失败不会退回共享宿主网络。relay 和网络进程在取消、父进程退出及启动失败时收束。Host 的无 Key 管理 API 仅允许 loopback 监听，非 loopback 必须配置 API key；模型控制通道与任务授权分开。

`credential_request` 只发起 owner/Session/Turn、已安装 Plugin、server 和声明字段绑定的专用输入。Web 的 password 控件直接提交 Host，值不进入聊天、工具结果或 Goose 历史；Host 私有 600/700 文件保存凭据，下一 Turn 向该 owner 的正式 MCP 配置注入。取消、删除、过期、版本或授权变化拒绝旧输入，返回 `no-store`/`no-referrer`。`oauth_authorize`/`oauth_status` 复用 Library OAuth transaction 和原 token 流程；Pilot 来源另持关联事实，停止或删除后迟到 callback 不交换 token，也不降级成无来源旧流程。

Pilot 接纳在 Session gate 内保存 Turn、冻结挂载并登记取消；环境探测、OAuth、MCP 启动与 Goose 执行在 gate 外进行。结束前显式关闭持有的 MCP 端口。Goose Bridge 先等待实际工具收束，再通过 Axum graceful shutdown 关闭 HTTP 连接、调用固定 rmcp 的公开 Session 关闭接口并等待 handler 释放引用；abort 仅用于失败兜底，关闭失败保留不确定结果。节点启动取消先通知资源所有者再等待清理，不从外层直接丢掉正在启动 MCP 的 future。

Pilot 清理保存 format 1 的不可变归属绑定 `state/platform/pilot-resources/<scope-hash>.json`，包含 owner、Session、创建时间、workspace 与 Goose 根。接纳和删除都在 Session gate 内重新核对当前行，防旧请求删除或接入同 ID 重建的 Session；删除持 Goose 原生 execution lease，仅清理对应 workspace、原生历史、候选准备与 mount 快照，成功后再删 Session 行，部分失败保留归属以供重试，已安装 Plugin 不随 Session 删除。mount 快照保持旧路径编码，按真实 Turn 集合与 format 1/2 JSON 中精确的 Session/Turn 身份逐文件回收，路径及读取不跟随符号链接；其他 Session、未知文件和归属不符的快照保留，只有空目录可移除。Graph 会话另用 `state/conversation-resources/<scope-hint>.json` 保留 Graph/Session/node 的共享 scope 归属；幸存 Run 或 metadata-only handoff 保留引用，最后引用释放时在原生 lease 内清理。Run lineage 墓碑仍只解释缺失前驱，不作为 Goose 恢复游标。

普通助手由 Graph 执行。可信入口绑定来源、Session、Graph 与结果节点；同 Session 串行，不同用户独立。既有一次性助手仍逐条创建 Run，结算旧 Run 后接续原生历史，并在新工作区只读挂载 `/previous`；逐节点会话 scope 不跨用户、Graph 或节点复用。

### 常驻助手实例

已接入的常驻路径是普通三节点循环：`wait_input (Op.host session.wait_input) → assistant (Agent) → reply (Op.host session.reply) → wait_input`。示例 [wecom-persistent-assistant.json](../examples/graphs/wecom-persistent-assistant.json) 须显式选择，不原地替换已有 `wecom-assistant`。Host 当前只接纳这一输入驱动、无有限轮次上限的助手形状，不建设 WeCom 专用 Runner。

- 首条可信消息惰性创建该 Session 的 current assistant Run，后续输入复用它；Session 保存 Turn、幂等、历史 binding 和 retire 事实，RunStore/Runner 拥有冻结 Graph、cursor 与执行状态，Host 工作区适配层保存 Run 的稳定节点 workspace 绑定。
- `wait_input` 从持久输入事实领取 Turn，按等待 invocation 幂等绑定，重试不取下一条；将本轮文本、可信身份与附件引用提交给 Agent。后续消息不覆写 `Run.input`，等待期间不调用模型。
- 新输入打断旧轮而非停止整个 Run。Host 取消并等待 Goose 与工具执行者真正退出，保存未提交现场 checkpoint 和持久 `Yielded` 事实；Runner 冻结 `Interruption` 控制 Artifact 后沿普通 route 到 `reply`，旧回复记 `suppressed`，再回到等待节点。它不把旧 Agent/Turn 标成成功，不自动重放未知业务效果。
- `reply` 读取本 Turn 绑定的确切 Agent commit；出站图片按 Turn 登记，不取长期 Run 的“最新回复”，不串到下一轮。Turn 结算不等待 Run `Completed`，平台发送 claim、ACK 与 unknown 仍复用既有账本。

Session SQLite 正式迁移至 v7：移除入站 Run 的全局唯一约束，为助手增加按 Run 保留的历史 binding、每 Session 一个 current 的部分唯一约束和 `(run, wait invocation key) → Turn` 领取事实。旧 `associate_channel_run` 仍拒绝一个 Run 关联多 Turn；只有显式助手 binding 和领取契约授权复用。迁移保留旧入站、Turn、投递和关联事实，生产升级前仍须备份，未对现有生产数据库执行迁移。

Run 详情和 Web 节点投影将 `RunResult.interruption` 显示为“已中断”，不标作成功提交或普通执行失败。新版初始化遇前驱的中断控制 Artifact 时读取其已收束的工作现场快照，不把控制 Artifact 当作业务文件提交。

运行看板按事实分段投影。普通 Run 仍画自身 `started`..`updated`（仍在执行时延伸到当前时刻）的一段；常驻实例的 Run 跨多个 Turn，因此 `/timeline` 另发 `activity`：`turns JOIN turn_runs` 的执行窗口（`created_at`/`updated_at`，合并相邻窗口，按本页过滤；只有 Turn 仍在跑且宿主仍持有该 Run 时才标记未结束），空闲等待不画成执行。常驻 Run 即使没有窗口也带空 `activity`，前端只在 `started` 处画标记，绝不把它拉到当前时刻；`started` 早于本页但本页有窗口的常驻 Run 仍会收录，Session 库不可读时降级为不带窗口。该投影只读、不带 prompt 或投递内容，不新增 schema，也不改变 `/runs` 列表字段。

常驻媒体授权来自可信 Turn 及固定 invocation binding：当前附件只读挂载 `/in/channel`；在本轮输入之前、最近一次 confirmed 投递之后的中断输入中，冻结最多最近 8 条快照，分别只读挂载 `/in/channel-pending/<turn>`。这是有界补充范围，不是全历史授权；后续已越过 confirmed 边界的轮次不再自动挂载这些附件。

首次 wait binding 查询最近 9 条以检测截断，实际只授权最近 8 条并保存 `pending_truncated`；当 `output.interrupted_messages_truncated=true`，Agent 必须如实说明更早中断输入未自动纳入/未完整阅读，不能声称信息全保留。该标记无需额外数据库 schema 迁移，旧 binding facts 缺省为 `false`。

停止/恢复保留 current binding；停止请求须等到执行者退出和真实 `Stopped`，重启仍需显式 resume，不因新消息或连接恢复自动执行未知效果。`GET /channel-sessions/{session}/assistant` 读取 current 实例；`POST /channel-sessions/{session}/assistant/retire` 接收 `{ "run_id": "…" }`，仅在 Run 已真实 `Stopped` 或终态且非 active、Session 无 Running Turn 和 pending/sending/unknown delivery 时允许退役。retire 保留历史但解除 current，禁止该 Run resume/重新绑定；下一条可信消息才惰性创建新实例。Graph 更新不热改旧 Run 快照或权限，合法新版经退役后的新实例接纳。

Graph cascade 删除先保护 active/未收束执行、共享原生历史与未结算投递，再退役目标助手；清理只按 owned Run/Turn/invocation binding 删除输入、回复、yield、checkpoint、工作区和 Artifact。它不是全 workspace-root 扫除或生产旧数据迁移，部分删除重试保留可核查的所有权事实。

会话 Run 也可单独删除，但按血缘设前置条件：不能是 current assistant binding（须先 retire），不能有未结算渠道投递，且每个更新的 Run 都必须已经不可能再执行（终态，或已有自己的后继因而无法 resume/recover）或已完成会读取它的首轮 invocation（继承现场、`/previous` 挂载与 Goose 原生前驱身份都发生在那一次执行里）。删除哪个环节不限顺序：删除前先落不可变墓碑事实 `state/run-deletions/<run>.json`（Run、Graph bundle 身份、Session、回复节点，以及被删 Run 自己当时的上一个 Run），lineage 遍历遇到缺失前驱时用同一会话的墓碑继续往后走，幸存历史因此仍是一条链、只有一个头，没有墓碑的缺失前驱仍然 fail-closed。若每个幸存环节都已是某个已删除环节的前驱，则该 Session 没有活动链，下一条被接纳的轮次重新开链。墓碑不随 Run 删除移除，也不参与 current 或恢复判定；单 Run 删除不隐式 retire 当前助手实例。共享的 Goose/legacy scope 在不再被幸存 Run 或交接引用时按独立归属事实清理，删除全部 Run 后同 Session 的下一次接纳建立新链。

### 渠道适配与监听

`call.session` 继承的是绑定用户的会话与显式输入映射，持有稳定来源身份。未知发送效果由 Agent 核查，ACK 与效果事实分别保存。Host 按 Graph 的 `channel.json` 自动监管原生 WeCom Gateway，私有控制路径由 state root 派生。Gateway 自己承担平台媒体边界：入站 image/file/voice 回调在本进程内下载短时 URL 并做 AES-256-CBC 解密，只把解密后的规范化附件交给 Host 既有契约（Host 冻结 SHA-256/大小/MIME 并只读挂载 `/in/channel`），临时 URL 与 AES key 不进入 Host 或 ledger；出站图片使用 Host 已校验的 `channel-replies` 项，由 Gateway 三阶段上传换 `media_id` 后单独发出 image 消息，发送前 durable claim、未确认结果不重发。出站任意文件类型平台未证实、不支持 video 回调，入站真实公网投递与文档理解仍需独立验收。

Gateway 常驻连接与 Graph Run 的执行状态分别投影。`GET /graphs` 只为实际受监管且被配置为 WeCom 事件目标的 Graph 返回 `listener`，通过鉴权的私有 NDJSON `status` 请求读取连接状态；已认证、连接中、认证中、重连、停止和不可用有独立状态，读取超时或异常不会冒充在线，也不返回控制凭据。`active_nodes` 来自仍有宿主执行者的 Running Run 游标及正在执行的并行分支，不把遗留游标当成当前工作。

Web 编辑画布的监听入口和工作流列表在已认证且空闲时使用 1px 虚线，并标注“常驻监听”；实际执行节点及活动工作流使用 3px 实线，空闲后恢复。重连或状态不可用显示对应文字与普通细实线。运行详情继续展示所选 Run 的历史事实；网关连接本身不创建 Run，惰性创建的常驻 Run 在 `wait_input` 时可以仍为 Running，但等待节点不计入 `active_nodes`，也不触发模型调用。

Pilot 的 `channel_configure`/`channel_enable` 经原生确认管理 owner-bound 渠道绑定，专用凭据输入复用上述机制。同一配置供 Supervisor、WeCom 入站和 Graph listener 使用；外部部署字段只读，单个 Gateway owner 冲突拒绝。空 users 默认拒绝所有用户，send_users 必须显式声明。停用立即撤销 callback key，重新启用轮换；回调授权仅限事件、单条进度查询和结算，不授予管理 API、任务联网或 Plugin 挂载。渠道控制能力已接入，真实企微收发仍须独立验收。

手动触发、计划、Webhook 与 Responses 子集共用接纳。Pilot 的 schedule 创建、列出、修改和删除复用同一 ScheduleStore 与 Dispatcher，按 owner 校验 Graph 和触发规则，不另建调度器。Responses 只实现文档中的子集；健康 `/health` 与就绪 `/ready` 不发送模型请求。API-key 和 Graph/工具授权由 Host 强制执行，Plugin 说明和提示词不授予权限。

WebUI 的 Anchor API key 只保存在当前标签页。输入框先检查请求头可接受的格式，再通过只读 `GET /graphs` 验证；成功才保存并关闭，错误 key、格式错误或服务异常保留输入框并分别提示。JSON 请求与 Pilot SSE 共用缓存恢复：含非 ASCII 字符、空格或控制字符的无效缓存不会进入请求头，401 后重新输入；较早请求的迟到 401 不丢弃其他请求刚验证成功的新 key。取消验证会中止请求，迟到结果不保存。

## 发行与验收边界

source-free builder 校验固定 Goose、目标 ELF、Graph 闭包、资源摘要与非密钥内容。它支持编译 Web 资产，拒绝脚本/source map、符号/硬链接和未声明资源；打包不会安装依赖或转换用户状态。使用命令见 [发行 builder](../rust/anchor-distribution/README.md)。

完整平台发行使用 format 2、kind `platform`，没有占位业务 Graph，包含 Host、固定 Goose、relay、网络 sidecar、Web、原生 network Plugin、uv/rustup-init 和 launcher；独立 Graph 格式仍为 format 1。安装器验证摘要和闭包，从私有快照解包，原子发布且拒绝覆盖现有安装。模型输入作为有限字段的数据解析，不执行 shell；launcher 清空继承环境并使用独立 HOME 和数据根，不借开发机解释器或缓存。当前平台要求 Linux x86_64、Bubblewrap、slirp4netns、git/sh/cc、CA 及对应动态库；Python、Rust 和包在安装后按需准备。脚本入口见 [平台交付](../rust/anchor-distribution/README.md#fresh-platform-installation)，验收与部署状态分别以台账为准。

常规 [小型 Graph 回归](runtime-contract-tests.md) 执行实际 Host、Runner、Goose、授权 MCP 与 Sandbox，只替换模型传输。检查最终结果、逐节点历史、workspace、Artifact、恢复与幂等；[候选回归](rust-production-candidate.md) 使用 release Host/Web/Goose 的隔离发行包。

常驻循环、实际 WeCom transport 和外部业务分别验收，具体范围与证据见 [企微助手验收边界](wecom-assistant.md#docmost-和验收) 与 [验收摘要](pilot-development-plan.md)。

候选代码和本地证据不等于生产已切换。外部业务、目标发行版、旧数据/配置迁移与回滚的未关闭范围统一见 [验收摘要](pilot-development-plan.md#保留的验收边界)。早期实现和设计快照见 [历史归档](archive/README.md)。
