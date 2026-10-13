# Plugin

Anchor Plugin 以安装后的目录为资产。安装器可接收社区来源并放入共享 Library；运行时读取 bundle 根目录 `plugin.json`。来源格式只影响安装时怎样定位清单，其他资源保持来源结构。

```text
library/plugins/<id>/
├── plugin.json
├── skills/<skill>/SKILL.md
├── .mcp.json                 # 可选 MCP 声明
└── 其他来源资源
```

Graph 的 AgentNode 保存 Plugin 目录 ID。Library 校验清单与 Skill 路径，Host 在节点调用时按当前安装内容只读挂载到 `/plugins/<id>`，实际依赖视图记入 Run 观察；升级从下一次节点调用起生效，已提交步骤不重放。Plugin 文件由安装流程或操作员维护，Graph 无权添加任意宿主路径。

## 工具与模型边界

Rust Host 使用 RMCP 读取 MCP server 的真实工具清单和 schema，并通过 Goose ACP/MCP 接入 AgentNode。工具采用 `<plugin>-<server>_<tool>` 命名，Skill 路径加入节点说明。Goose 负责模型循环、Provider、工具交互、原生会话和 compaction；Anchor 负责 Graph、Run、Artifact、Sandbox、Plugin 和宿主权限事实。

stdio server 在现有 Bubblewrap 节点边界中运行，Plugin 挂载只读；HTTP MCP 要求节点 `network:true`，凭证与 headers 由声明在绑定时展开。模型不能提供新的 host path、服务凭证或授权。节点完成使用受限 `final_result`，其路由与工具结果由宿主校验，不把普通文本当作成功完成。

当前兼容根清单、Skill/资源读取、只读挂载和 MCP；来源中的 `hooks`、`commands`、`agents` 不作为额外执行入口。当前产品不依赖旧 Tool Search、CodeMode 或另一套 Agent loop。

安装没有 HTTP 入口：Plugin 只能由管家走 `plugin_install` 的人工确认流程安装（见下节）；插件页只读，不提供安装表单。操作员 CLI（`anchor-library --root <Library> install --directory <候选目录> --id <id>`）**拒绝**安装声明了运行期依赖的插件——它没有求解依赖、落地视图的宿主能力，装了也起不来；`--review` 仍可完整查看该候选。Plugin 自己声明运行期依赖时，宿主在安装时按部署授权的来源求解并落地到共享存储，再为该插件版本生成视图；求解或取包失败就不装，并把工具原文报给人。声明写在清单顶层：`"dependencies": {"python": ["idna==3.6"]}`（清单顶层字段是**封闭白名单**：`name`/`description`/`version`/`author`/`homepage`/`repository`/`license`/`keywords`/`interface`/`skills`/`mcpServers`/`dependencies`/`network`/`mutable`/`hooks`/`commands`/`agents`/`apps`，其他键一律拒绝；这是有意的，宁可让作者发现问题，也不要静默忽略一个拼错的字段），要用视图的解释器就把 server 的 `command` 写成 `${ANCHOR_PLUGIN_VIEW}/bin/python`。插件自己的可变数据用顶层 `"mutable": ["data/", "cache.db"]` 声明（相对插件目录、不含 `..`、不能覆盖 `plugin.json`、`.mcp.json` 或外置 MCP 声明文件；末尾 `/` 表示子树）：这些路径是插件进程**唯一**可写的地方，平台从不读、不复制、不覆盖它们，代码摘要与插件包清单也不包含它们——同一份代码无论数据长到多大，摘要都不变。插件只有一份（`plugins/<id>/`），Graph 包只写 `{"id": …}` 引用：升级插件不需要改包，正在运行的 Run 从**下一次节点调用**起用新代码，已完成步骤不回滚、不重放，它实际用过的视图记进 Run 记录作为观察。OAuth 从 Plugin 面板显式授权，owner-bound token 保存在操作员 Library，凭证不进入 Graph 包。授权、刷新与 MCP 连接有确定性本地测试；真实 provider 和回调部署需独立验收，不能声明所有社区 server 兼容。

## Pilot 任务联网

插件清单顶层 `"network": true` 声明联网需求，远程 HTTP/SSE MCP 自动产生需求。已安装的 `network` Plugin 可以主动启用，也会由学术调研等已挂载 Plugin 依赖启用；依赖关系不额外写挂载开关。Session 的明确禁止联网优先，stdio Sandbox 与宿主侧 HTTP/MCP/OAuth 都执行该权限，同一回合使用冻结快照。网络能力、工具与独立 Graph 的兼容边界见 [任务联网能力](network-plugin.md)。

## 官方工具和外部工具环境

官方学术 CLI、Docmost 附件 MCP、WeCom 应用 MCP 和机器人 Gateway 都是 Rust 二进制。Docmost Plugin 的远端 HTTP MCP 使用 `DOCMOST_API_KEY`；本地 `anchor-docmost-tools` 只上传授权的 `/in/publish/assets` 普通文件，不绕过发布门禁。

操作员仍可登记第三方工具的显式 entrypoint、environment 和 imports。Host 保留解释器符号链接、版本化解释器、显式环境路径及 PYTHONPATH 等外部格式兼容；使用其他语言的第三方工具由部署者提供其运行环境。这不增加官方构建、运行或回归的解释器依赖。环境必须由操作员稳定维护，Graph 与 Plugin 不自行安装依赖或扩大挂载。

## Pilot 获取、开发与确认后安装

用户要求获取或开发 Plugin 后，Pilot 可以先用 `plugin_list`、`plugin_read` 判断现有能力，再在自己的 Session 工作区中下载源码、编辑和构建。普通 Turn 与 `/v1/responses` 共用 `anchor_run`、`anchor_read`、`anchor_edit` 和同一个接纳路径；Responses 的 `previous_response_id` 继续原 Session，独立请求使用独立工作区。候选工具的目录都相对 `/workspace`，跨 Turn 保留，绝对路径、`..` 和越界符号链接被拒绝。部署 workspace 根与 Goose 内部目录不当作候选根。

审查、准备和导入都只暂存代码；源目录的 `mutable` 数据不会进入候选或正式包，数据位置初始为空。升级保留正式 Plugin 的已有数据。`anchor_read` 按行分页，每页文本预算 256 KiB；单行超限时使用 `anchor_run` 查看有界字节范围。

1. **获取与适配**：按用户的仓库或 HTTP 来源实际获取内容，保存固定 commit 或内容摘要；读取入口、清单、源码与配置需求，收敛到 Host 支持的 MCP 字段。`plugin_read` 披露已安装 Plugin 的资源、依赖与脱敏审查信息，可读受限源码资源；不直接读取原始清单中可能包含的凭据值。
2. **准备与自测**：Python 依赖写在清单顶层 `dependencies.python`，调用 `plugin_prepare` 后再用 `plugin_test` 启动真实 MCP，并按可选 `server`、`tool`、`arguments` 进行业务调用。返回区分准备完成、协议启动、被测摘要和 `business_tested`，工具目录不冒充业务通过。候选准备根位于所属 Pilot scope 的 `candidates/`，临时只读视图引用 Library 共享包，不写正式 Plugin 视图、不借项目私有 venv；候选修改后按新摘要重新测试。测试不能从任意环境变量、OAuth 或清单声明获取宿主凭据。
3. **审查**：`plugin_review` 或操作员 CLI 的 `install --review` 返回相同内容指纹、文件清单、每个 server 的传输/入口、环境变量名称及内联值、缺少源码等告警，审查不发布 Plugin。编译产物同时带可读源码、锁文件与构建说明，入口用相对 `bin/...`，不引用开发目录。
4. **安装与确认**：`plugin_install` 只接受 `{directory,id}`，模型不能填写确认字段。原生表单绑定候选摘要、创建/覆盖、工具与依赖；取消、忽略、超时都不安装，同一摘要已安装且依赖可用时直接返回现状。真正发布前持 catalog guard 与 Library lease，核对现有版本、复制并校验宿主私有快照；依赖准备和最终发布只读该已确认快照，原候选在等待期间变化被拒绝，在快照之后变化不会替换本次批准内容。缺少源码的编译产物仍需操作员明确接受。
5. **挂载与核查**：安装不自动挂载，也不把候选工具加入当前 Turn。用户在插件面板挂载后，从下一 Turn 生效；用随机或实际输入调用正式工具验证结果。正式包及依赖视图不引用候选准备根，删除开发目录或所属 Session 后仍可运行。

任务下载、git、Cargo、依赖请求与候选 MCP 共用当 Turn 冻结的网络权限；候选 `network:true` 不给自己授权，Session 明确禁止优先。禁止联网时可使用本地已缓存的包和工具链，需要取包则报告缺口。取消涵盖命令、MCP 初始化/业务调用、依赖 HTTP 探测/解析和安装锁等待；Host 启动的命令及 stdio MCP 进程需结束并等待回收，HTTP MCP 取消请求等待而不保证远端进程退出。恢复先核查保存的工作现场和工具结果，不自动重放旧确认。

Python 候选代码复用 Anchor 唯一配置的 `ANCHOR_DEPENDENCIES_PYTHON=/absolute/prefix/bin/python3`，例如 3.12；部署者另设 `ANCHOR_PILOT_PYTHON_ENABLED=1` 才授权 Pilot 执行它。Host 校验 Python 3.8+，只读挂载安装前缀；Pilot 的 Python 别名始终使用当前 Turn 冻结的共享解释器，项目命令通过自己的 venv 使用它。单独配置解释器不启用依赖准备；准备仍使用获授权的固定 uv 与既有 Runtime 配置。正式第三方包声明在候选 `plugin.json` 的 `dependencies.python`，由部署配置的来源与共享存储/Plugin view 处理。宿主 Python 不能被候选、开发 pip 或 Graph 改写。

Python 开发按项目使用 `development_prepare`：只传 `project` 默认创建空私有 venv 与 pip，已有私有环境保留试装包。随后所有安装、调试和测试命令都用 `anchor_run(project=...)`，例如 `pip install -r requirements.txt`、`pip install -e '.[dev]'`、`python -m pip check` 和 console script。`python`、`python3`、对应版本别名、裸 pip 和 shell 使用同一个 `/development/env`；项目文件由 pip 正常解析，不复制成 Host 长期声明。内联 `requirements`/`development_requirements` 数组是显式安装，不删除未列出的包。`action='rebuild'` 重建空环境、保留源码，依赖另行安装；`action='discard'` 只释放私有环境，可在删除项目源码前调用。

准备和全部项目命令串行，等待可取消；失败、超时或取消后的核查提示跨重启保留，先检查包、源码和测试再继续。默认准备只证明入口与绑定可用，不证明依赖健康。旧项目默认继续使用只读视图，返回 `legacy_read_only` 和 `pip_available=false`，首次显式 rebuild 成功才切换；不自动迁移或删除旧引用。若只剩解包 store 而无 wheel/源码，旧视图仍可用，私有空环境/pip 可重建，但包需重新获得安装来源。普通开发 pip 服从本 Turn 网络及沙箱文件权限，不能借宿主网络或写正式存储。完整契约见 [开发环境方案](development-environment-freedom-plan-revised.md)。

Rust 由部署者同时设置 `ANCHOR_PILOT_RUSTUP`、`ANCHOR_PILOT_RUSTUP_HOME` 和 `ANCHOR_PILOT_CARGO_HOME`，最后一项应是已存在的缓存根。Host 只读挂载 proxy、共享工具链与 registry/git 缓存；可信 Cargo 入口在 Session 工作区建立可写 `CARGO_HOME`，首次在沙箱内复制缓存，用文件锁保护并发及取消后的重试，不复制编译器，不挂宿主 Cargo 凭据。开发上下文提供 PID namespace 内私有 proc，默认 Cargo、`cargo +<已装版本>` 和 shell 编排都可直接构建；只有联网允许下的精确直接 `rustup toolchain install` 获取工具链写权限，并强制 `--no-self-update`。不改共享 default，不自建版本管理或 Rust 运行期视图；`Cargo.toml`/`Cargo.lock` 管理 crate，产物是普通 Plugin 二进制。Pilot 命令可请求最多 600 秒，AgentNode 仍按原来的 30 秒上限执行。

`DELETE /sessions/{id}` 只回收有明确 owner/Session/创建时间绑定的 workspace、Goose 原生历史、私有项目环境、候选准备与 mount 快照，先取得原生执行 lease，再删除 Session 事实；Running Turn、活执行 lease 或归属不明时拒绝，部分失败保留事实供重试，正式 Plugin 保留。快照按真实 Turn 与 JSON 身份核对后逐文件删除；Session ID 编码碰撞时保留其他 Session 和无法证明归属的文件，不整目录清空。两入口、共享工具链与清理的实际验收见 [开发台账](pilot-development-plan.md)，实现边界见 [当前架构](architecture.md#session渠道与入口)。

管家不能修改或删除定义它自己的 Graph，也不能安装为它自己提供指令的 Plugin；操作员接口仍承担这类维护。外部 `tool.json` 登记接口、通用市场与对外发布未在此专项新增。完整作者契约见 [`plugin-authoring/SKILL.md`](../plugins/anchor-steward/skills/plugin-authoring/SKILL.md)，专项目标与接续约束见 [开发计划](pilot-plugin-authoring-development-plan.md)。

## 企业微信

`plugins/wecom` 的应用 API MCP 入口是 `bin/anchor-wecom-tools`，提供 `wecom_send_text`、`wecom_send_markdown` 和 `wecom_get_user`。通道声明 `channel.json` 指向 `bin/anchor-wecom-gateway`；Host 的 ChannelSupervisor 按服务生命周期监管唯一网关，不在每个 Run 中新建平台连接。

Bot 长连接凭证与应用 API 凭证分开配置；普通助手 Graph 按成员允许名单接收私聊，Host 保存每位用户的 Session/Turn 与取消事实。Gateway 保存 ACK、unknown 和 suppression 投递账本，不自动重发未知平台请求。当前 Gateway 的官方媒体 URL 下载、AES 解密和富媒体回传尚未完成公网验收；Host 的本地规范化附件契约不能替代这些能力。

安装、配置和验收边界见 [企业微信助手接入](wecom-assistant.md)、[原生应用工具](../rust/anchor-wecom-tools/README.md) 与 [Gateway](../rust/anchor-wecom-gateway/README.md)。真实服务和真实 Provider 的状态以当前开发台账的证据为准。
