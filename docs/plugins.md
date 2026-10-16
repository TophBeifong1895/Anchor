# Plugin

Anchor Plugin 以安装后的目录为资产。安装器可接收社区来源并放入共享 Library；运行时按 ID 从 Library 插件目录读取 `plugin.json`。来源格式只影响安装时怎样定位清单，其他资源保持来源结构。

```text
library/plugins/<id>/
├── plugin.json
├── skills/<skill>/SKILL.md
├── .mcp.json                 # 可选 MCP 声明
└── 其他来源资源
```

Graph 的 AgentNode 保存 Plugin 目录 ID。Library 校验清单与 Skill 路径，Host 在节点调用时按当前安装内容只读挂载到 `/plugins/<id>`，实际依赖视图记入 Run 观察；升级从下一次节点调用起生效，已提交步骤不重放。Plugin 文件由安装流程或操作员维护，Graph 无权添加任意宿主路径。

## 工具与模型边界

Rust Host 使用 RMCP 读取 MCP server 的真实工具清单和 schema，并通过 Goose ACP/MCP 接入 AgentNode。工具归属来自 Plugin/server 的实际装配，说明、Skill 和 schema 经固定发现入口按需读取。Goose 负责模型循环、Provider、工具交互、原生会话和 compaction；Anchor 负责 Graph、Run、Artifact、Sandbox、Plugin 和宿主权限事实。

stdio server 在现有 Bubblewrap 节点边界中运行，Plugin 挂载只读；HTTP MCP 要求节点 `network:true`，凭证与 headers 由声明在绑定时展开。模型不能提供新的 host path、服务凭证或授权。节点完成使用受限 `final_result`，其路由与工具结果由宿主校验，不把普通文本当作成功完成。

远程 HTTPS MCP 由 `anchor-mcp-host` 显式启用 RMCP 的 TLS 支持，使用系统信任证书并验证服务域名；不可信证书和域名不匹配均拒绝连接。能建立 HTTPS 连接不代表凭据有效，服务返回 401 时仍需修正该插件的凭据。

当前兼容根清单、Skill/资源读取、只读挂载和 MCP；来源中的 `hooks`、`commands`、`agents` 不作为额外执行入口。当前产品不依赖旧 Tool Search、CodeMode 或另一套 Agent loop。

## 发现与读取

Pilot 与 AgentNode 共用 `PluginDisclosureCatalog` 和 `ToolDisclosure`。`anchor_plugins` 搜索本次准备并获授权的 Plugin；`anchor_plugin_read` 按 section 读取概览、Plugin 使用说明、Skill/资源目录、单个 Skill/资源或安全的 server 声明。`anchor_tools` 搜索完整授权工具目录，`anchor_tool_detail` 返回所选工具的完整 description、参数 schema 和归属，再由 `anchor_tools_call` 调用。发现入口始终可见，普通提示也说明用法；`tom` 只承载短边界提醒，不保存完整目录，也不是发现能力的前提。Pilot 的 `plugin_list`/`plugin_read` 查询已安装 Library，执行发现查询本 Turn 的冻结挂载；管理查询不赋予调用权限。

同一份冻结目录还派生不超过 16 KiB 的能力摘要，通过聚合 MCP 的 `ServerInfo.instructions` 进入 Goose 原生扩展系统提示：逐项提供 ID、名称、用途简述、准备状态、实际工具数量和读取入口，不附完整 Skill、资源、schema 或凭据。名称和描述分别取首个非空行、最多 160 字符，并以 JSON 编码；预算之外返回 `total`/`shown`/`omitted` 与完整检索入口。元数据不是执行指令，未显示不代表不可用；`available` 说明定义可发现/读取，实际调用仍取决于当前授权工具。正常推理和压缩后的继续请求都由 Goose 携带当前摘要，框架内部专用摘要请求使用自己的提示，不在此承诺内。新 Turn 或原生 Session load 后重新装配、校准扩展，当前摘要替换旧摘要，原生历史仍保留；直接 schema 和渐进 schema 模式均适用，关闭 `tom` 不影响摘要。

目录默认 20 项、最多 100 项，同时限制为 16 KiB JSON 页，并返回 `next_offset`；检索为完整 Plugin/工具元数据和 Skill 头部的大小写无关多词 AND 子串，支持中文、精确 ID 和分页兜底。显示摘要限 160 字符，检索不使用裁短后的摘要。选中详情限 256 KiB JSON，工具 schema 超限明确报错，不伪装成完整结果。UTF-8 文本文件最多 1 MiB，按字节 offset 分页（默认 16 KiB、最大 32 KiB），返回 SHA256；续页传 digest 检测正文热更新，以 `next_offset=null`/`end_of_file=true` 结束。压缩后按相同入口重读，继续使用 Goose 原生 compaction，不增加 Anchor 记忆或上下文系统。

读取配置文件的分类来自已验证、规范化的清单路径；隐藏文件、`plugin.json`、MCP 配置与 `channel.json` 不由披露工具返回。执行读取在 Library 非阻塞共享 guard 内核查当前授权摘要及公开文件分类，只读准备时与当前公开路径的交集，然后安全打开文件；正式发布持同一排他 lease。发布忙时明确拒绝本次读取以供重试，授权变化要求新的 invocation，普通正文更新仍可读。Pilot 管理入口同样在 guard 内读取定义及 Skill 元数据。披露目录是可重建投影，不新增持久资产、Plugin 历史副本或授权源。

`anchor_tools_call` 在 Bridge 保存观察前解析真实目标和参数，保留外层 ACP 原生调用身份，继续交给原授权端口。Pilot 保留 native peer/elicitation，Node 保留真实工具观察、媒体、错误、回执与取消收束；递归元工具及间接 `final_result` 被拒绝。工具 schema 的披露沿用 `ANCHOR_NODE_TOOL_DISCLOSURE`：`1` 强制、`0` 保留直接 schema 模式，未显式设置时隐藏工具超过 3 个自动启用；Pilot 固定管理工具继续直接可见。目录与单项读取入口不依赖此开关。MCP 仍按现有准备流程连接，未增加懒连接或 Code Mode，实际通过范围见 [验收摘要](pilot-development-plan.md)。

## 安装与更新

插件库页面支持从 GitHub tree URL 或本地插件目录预检、安装与更新；确认使用同一个应用安装前置条件和 Library 发布锁，Pilot 的 `plugin_install` 保留原生确认入口（见 [开发与交付](pilot-development.md#候选准备测试与交付)）。本地导入只接受普通文件列表，最多 2048 个文件、解码后合计 32 MiB；联网获取源码或依赖需要明确授权。卸载展示会话/未结束 Run 的阻止原因及 Graph 影响，并确认删除插件自身数据。操作员 CLI（`anchor-library --root <Library> install --directory <候选目录> --id <id>`）**拒绝**安装声明了运行期依赖的插件——它没有求解依赖、落地视图的宿主能力，装了也起不来；`--review` 仍可完整查看该候选。Plugin 自己声明运行期依赖时，宿主在安装时按部署授权的来源求解并落地到共享存储，再为该插件版本生成视图；求解或取包失败就不装，并把工具原文报给人。声明写在清单顶层：`"dependencies": {"python": ["idna==3.6"]}`（清单顶层字段是**封闭白名单**：`name`/`description`/`version`/`author`/`homepage`/`repository`/`license`/`keywords`/`interface`/`skills`/`mcpServers`/`dependencies`/`network`/`mutable`/`hooks`/`commands`/`agents`/`apps`，其他键一律拒绝；这是有意的，宁可让作者发现问题，也不要静默忽略一个拼错的字段），要用视图的解释器就把 server 的 `command` 写成 `${ANCHOR_PLUGIN_VIEW}/bin/python`。插件自己的可变数据用顶层 `"mutable": ["data/", "cache.db"]` 声明（相对插件目录、不含 `..`、不能覆盖 `plugin.json`、`.mcp.json` 或外置 MCP 声明文件；末尾 `/` 表示子树）：这些路径是插件进程**唯一**可写的地方，平台从不读、不复制、不覆盖它们，代码摘要与插件包清单也不包含它们——同一份代码无论数据长到多大，摘要都不变。插件只有一份（`plugins/<id>/`），Graph 包只写 `{"id": …}` 引用：升级插件不需要改包，正在运行的 Run 从**下一次节点调用**起用新代码，已完成步骤不回滚、不重放，它实际用过的视图记进 Run 记录作为观察。OAuth 从 Plugin 面板显式授权，owner-bound token 保存在操作员 Library，凭证不进入 Graph 包。授权、刷新与 MCP 连接有确定性本地测试；真实 provider 和回调部署需独立验收，不能声明所有社区 server 兼容。

## Pilot 任务联网

插件清单顶层 `"network": true` 声明联网需求，远程 HTTP/SSE MCP 自动产生需求。已安装的 `network` Plugin 可以主动启用，也会由学术调研等已挂载 Plugin 依赖启用；依赖关系不额外写挂载开关。Session 的明确禁止联网优先，stdio Sandbox 与宿主侧 HTTP/MCP/OAuth 都执行该权限，同一回合使用冻结快照。网络能力、工具与独立 Graph 的兼容边界见 [任务联网能力](network-plugin.md)。

## 官方工具和外部工具环境

官方学术 CLI、Docmost 附件 MCP、WeCom 应用 MCP 和机器人 Gateway 都是 Rust 二进制。Docmost Plugin 的远端 HTTP MCP 使用 `DOCMOST_API_KEY`；本地 `anchor-docmost-tools` 只上传授权的 `/in/publish/assets` 普通文件，不绕过发布门禁。

操作员仍可登记第三方工具的显式 entrypoint、environment 和 imports。Host 保留解释器符号链接、版本化解释器、显式环境路径及 PYTHONPATH 等外部格式兼容；使用其他语言的第三方工具由部署者提供其运行环境。这不增加官方构建、运行或回归的解释器依赖。环境必须由操作员稳定维护，Graph 与 Plugin 不自行安装依赖或扩大挂载。

## Pilot 获取、开发与交付

Pilot 在 Session 工作区获取和编辑候选，经 `plugin_prepare`、`plugin_test`、审查和原生确认安装；正式包与依赖视图独立于开发目录。私有 Python 环境、Rust 工具链、冻结权限、取消与清理的唯一操作契约见 [Pilot 开发指南](pilot-development.md)。

管家不能修改或删除定义它自己的 Graph，也不能安装为它自己提供指令的 Plugin；操作员接口承担这类维护。完整工具格式见 [作者 Skill](../plugins/anchor-steward/skills/plugin-authoring/SKILL.md)。

## 企业微信

`plugins/wecom` 的应用 API MCP 入口是 `bin/anchor-wecom-tools`，提供 `wecom_send_text`、`wecom_send_markdown` 和 `wecom_get_user`。通道声明 `channel.json` 指向 `bin/anchor-wecom-gateway`；Host 的 ChannelSupervisor 按服务生命周期监管唯一网关，不在每个 Run 中新建平台连接。

Bot 长连接凭证与应用 API 凭证分开配置；普通助手 Graph 按成员允许名单接收私聊，Host 保存每位用户的 Session/Turn 与取消事实。Gateway 保存 ACK、unknown 和 suppression 投递账本，不自动重发未知平台请求。当前 Gateway 的官方媒体 URL 下载、AES 解密和富媒体回传尚未完成公网验收；Host 的本地规范化附件契约不能替代这些能力。

安装、配置和验收边界见 [企业微信助手接入](wecom-assistant.md)、[原生应用工具](../rust/anchor-wecom-tools/README.md) 与 [Gateway](../rust/anchor-wecom-gateway/README.md)。真实服务和真实 Provider 的范围见 [验收摘要](pilot-development-plan.md)。

## 界面与交互

Pilot、Graph 和 Run 共用插件卡片、搜索与详情：列表显示名称、用途和状态；标识、内容摘要、Skill、MCP 与使用说明放在详情中。手机以抽屉打开会话插件，详情支持键盘关闭并恢复焦点。

会话选中后预读一次插件列表，仅面板展开时继续轮询；反复开关共用未完成的请求并保留已确认列表。首次等待显示加载占位，读取失败可重试，收到空列表后才显示未安装；切换会话取消旧请求。Host 列表读取实时目录信息和当前依赖视图，不读取程序文件正文计算代码摘要；详情与执行继续读取完整定义，不缓存跨请求的权限或可用性。

列表先显示 50 个卡片，可继续加载；搜索和筛选覆盖完整目录，改变条件后重新从首批显示。插件数量和已挂载数量始终统计完整结果。

| 入口 | 操作及事实来源 |
| --- | --- |
| 插件库 | 安装/更新/卸载和当前凭据管理；预检、确认与执行结果分别显示 |
| Pilot 会话 | 开关提交挂载请求，以 Host 返回状态为准；仅操作后显示保存与接续反馈，不常驻解释内部 Turn 机制 |
| Graph 节点 | 复选框编辑插件引用；常驻助手经操作员保存后下一 Turn 生效，普通 Graph 用于后续 Run；查看详情不提交草稿 |
| Run 节点 | 只读展示保存的绑定；常驻助手显示当前或最近一轮冻结的插件，不读取当前 Library 补写历史，也不提供挂载或授权操作 |

联网插件的主动开关与依赖启用分别展示；联网许可缺失时显示未知并禁用许可修改。挂载请求可能在当前回复结束后继续原任务，网络许可修改只影响后续 Turn。OAuth 操作仅在当前库详情提供，读取失败显示未知，可重试；撤销失败保留最后确认的状态，授权窗口打不开时提供手动链接。

插件凭据详情只展示缺失、已保存、部署管理或授权声明失配，已保存不等于外部服务验证成功。详情页与 Pilot 使用同一 owner-bound 存储；独立保存只完成同一目标、仍有效的待填写请求，旧请求不会覆盖新值，已处理请求的历史保留。渠道页面复用相同凭据组件，旧 Run 详情不提供当前配置修改。
