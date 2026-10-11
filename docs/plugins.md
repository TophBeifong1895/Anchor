# Plugin

Anchor Plugin 以安装后的目录为资产。安装器可接收社区来源并放入共享 Library；运行时读取 bundle 根目录 `plugin.json`。来源格式只影响安装时怎样定位清单，其他资源保持来源结构。

```text
library/plugins/<id>/
├── plugin.json
├── skills/<skill>/SKILL.md
├── .mcp.json                 # 可选 MCP 声明
└── 其他来源资源
```

Graph 的 AgentNode 保存 Plugin 目录 ID。Library 校验清单与 Skill 路径，Host 在 admission 时复制并冻结资源，按节点只读挂载到 `/plugins/<id>`。Run 保存资源身份和摘要；后续 Library 替换不改变已有 Run 的冻结绑定。Plugin 文件由安装流程或操作员维护，Graph 无权添加任意宿主路径。

## 工具与模型边界

Rust Host 使用 RMCP 读取 MCP server 的真实工具清单和 schema，并通过 Goose ACP/MCP 接入 AgentNode。工具采用 `<plugin>-<server>_<tool>` 命名，Skill 路径加入节点说明。Goose 负责模型循环、Provider、工具交互、原生会话和 compaction；Anchor 负责 Graph、Run、Artifact、Sandbox、Plugin 和宿主权限事实。

stdio server 在现有 Bubblewrap 节点边界中运行，Plugin 挂载只读；HTTP MCP 要求节点 `network:true`，凭证与 headers 由声明在绑定时展开。模型不能提供新的 host path、服务凭证或授权。节点完成使用受限 `final_result`，其路由与工具结果由宿主校验，不把普通文本当作成功完成。

当前兼容根清单、Skill/资源读取、只读挂载和 MCP；来源中的 `hooks`、`commands`、`agents` 不作为额外执行入口。当前产品不依赖旧 Tool Search、CodeMode 或另一套 Agent loop。

安装没有 HTTP 入口：Plugin 只能由管家走 `plugin_install` 的人工确认流程安装（见下节）；插件页只读，不提供安装表单。操作员 CLI（`anchor-library --root <Library> install --directory <候选目录> --id <id>`）**拒绝**安装声明了运行期依赖的插件——它没有求解依赖、落地视图的宿主能力，装了也起不来；`--review` 仍可完整查看该候选。Plugin 自己声明运行期依赖时，宿主在安装时按部署授权的来源求解并落地到共享存储，再为该插件版本生成视图；求解或取包失败就不装，并把工具原文报给人。声明写在清单顶层：`"dependencies": {"python": ["idna==3.6"]}`（清单顶层字段是**封闭白名单**：`name`/`description`/`version`/`author`/`homepage`/`repository`/`license`/`keywords`/`interface`/`skills`/`mcpServers`/`dependencies`/`hooks`/`commands`/`agents`/`apps`，其他键一律拒绝；这是有意的，宁可让作者发现问题，也不要静默忽略一个拼错的字段），要用视图的解释器就把 server 的 `command` 写成 `${ANCHOR_PLUGIN_VIEW}/bin/python`。OAuth 从 Plugin 面板显式授权，owner-bound token 保存在操作员 Library，凭证不进入 Graph 包。授权、刷新与 MCP 连接有确定性本地测试；真实 provider 和回调部署需独立验收，不能声明所有社区 server 兼容。

## 官方工具和外部工具环境

官方学术 CLI、Docmost 附件 MCP、WeCom 应用 MCP 和机器人 Gateway 都是 Rust 二进制。Docmost Plugin 的远端 HTTP MCP 使用 `DOCMOST_API_KEY`；本地 `anchor-docmost-tools` 只上传授权的 `/in/publish/assets` 普通文件，不绕过发布门禁。

操作员仍可登记第三方工具的显式 entrypoint、environment 和 imports。Host 保留解释器符号链接、版本化解释器、显式环境路径及 PYTHONPATH 等外部格式兼容；使用其他语言的第三方工具由部署者提供其运行环境。这不增加官方构建、运行或回归的解释器依赖。环境必须由操作员稳定维护，Graph 与 Plugin 不自行安装依赖或扩大挂载。

## 管家拉取与人工确认后安装

第三方插件的实现经常不能「拉下来就能用」：清单字段、入口路径、凭证方式和依赖都需要适配。Anchor 的做法是**管家（常驻助手）在自己的工作区里完成拉取与适配，安装由人确认**。整件事分两半：前半段是管家自己的工作（用 `anchor_run`/`anchor_read`/`anchor_edit`，节点权限决定能否联网），后半段是宿主工具 `plugin_review` 与 `plugin_install`。

1. **管家准备**：拉取固定 commit、读懂清单与入口、把 `mcpServers` 收敛到 Host 支持的白名单、把凭证改成环境变量引用、需要时编译，并在沙箱里真的启动 MCP、调用只读工具自测。产物是工作区内一份自包含目录；编译产物必须同时带可读源码与构建说明。
2. **审查（只读）**：管家用 `plugin_review`（宿主工具）或操作员用 `anchor-library --root <Library> install --directory <候选目录> --id <id> --review` 得到审查单——内容指纹、文件清单、每个 server 的传输方式/命令/参数或 URL 与授权方式、需要的环境变量**名字**，以及「内联字面值」和「缺少源码的编译产物」告警。两条路径共用同一套校验，`--review` 不写 Library。管家只能用工作区内的相对目录，绝对路径、`..` 和指向工作区外的符号链接都被拒绝。
3. **安装（需要人确认）**：管家调用 `plugin_install`。它**只接受** `{directory, id}`，接受不了任何模型提供的确认字段；系统先把待批内容冻结成「指纹 + 新建还是覆盖 + 将执行什么 + 环境变量名 + 告警」，弹原生表单，由操作员点确认。取消、忽略、超时都算不批。答完还会再核对一次：候选或 Library 在确认期间变了就拒绝并要求重新确认（"你批的和装的不一样"装不进去）。同一指纹已安装时直接回「已是最新」，不打扰人。缺少源码的编译产物默认拒绝，除非表单里额外勾一次「接受无法阅读的产物」。
4. **留痕**：问题与答案本身是持久事实（指纹写在问题文本里），工具调用与结果也写进 Turn 的持久事实，因此「谁批的、批的哪份、装成的指纹」都可查。
5. **自升级禁止**：管家不能修改或删除定义它自己的那张图（`graph_create`/`graph_update`/`graph_delete` 均拒绝），也不能安装为它自己提供指令的 Plugin——这些只能由操作员用 `anchor-library install` 命令行完成。

管家节点需要的部署配置（均由部署者提供，Graph/Plugin 不自行扩大）：节点 `network: true`；一份操作员登记的工具环境（`tools/<id>/tool.json` 的 `environment`）提供 git、解释器与构建工具；绑定 `plugins/anchor-steward`（只含适配契约说明，没有 MCP server）以及需要的检索 Plugin。插件安装后按 id 绑定到节点，Run 冻结该 id 与摘要，之后 Library 更换不影响已有 Run。

尚未实现：环境（`tool.json`）的登记与校验接口（仍是文件系统）、依赖/包申请通道、管理动作的查询视图，以及交互层自升级守卫的端到端用例（当前只有逻辑与参数级测试）。适配契约的完整字段规则见 [`plugins/anchor-steward/skills/plugin-authoring/SKILL.md`](../plugins/anchor-steward/skills/plugin-authoring/SKILL.md)。

## 企业微信

`plugins/wecom` 的应用 API MCP 入口是 `bin/anchor-wecom-tools`，提供 `wecom_send_text`、`wecom_send_markdown` 和 `wecom_get_user`。通道声明 `channel.json` 指向 `bin/anchor-wecom-gateway`；Host 的 ChannelSupervisor 按服务生命周期监管唯一网关，不在每个 Run 中新建平台连接。

Bot 长连接凭证与应用 API 凭证分开配置；普通助手 Graph 按成员允许名单接收私聊，Host 保存每位用户的 Session/Turn 与取消事实。Gateway 保存 ACK、unknown 和 suppression 投递账本，不自动重发未知平台请求。当前 Gateway 的官方媒体 URL 下载、AES 解密和富媒体回传尚未完成公网验收；Host 的本地规范化附件契约不能替代这些能力。

安装、配置和验收边界见 [企业微信助手接入](wecom-assistant.md)、[原生应用工具](../rust/anchor-wecom-tools/README.md) 与 [Gateway](../rust/anchor-wecom-gateway/README.md)。真实服务和真实 Provider 的状态以当前开发台账的证据为准。
