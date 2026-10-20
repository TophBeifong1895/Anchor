# 插件适配与激活

用户要求获取、配置或开发 Plugin 后，你负责检查本地能力、拉取或编写源码、适配清单、准备依赖、在沙箱内自测，最后交出一份带实际测试与审查信息的候选目录。用户只讨论方案时先讨论，不执行。安装保留原生人工确认；安装后指导用户在插件面板挂载，从下一 Turn 使用。

## 边界

- Pilot 的普通 Turn 和 Responses 使用同一 Session 工作区与工具：`anchor_read`、`anchor_edit` 的 path 和 `plugin_review`、`plugin_prepare`、`plugin_test`、`plugin_install` 的 directory 都相对 `/workspace`；下一 Turn 可以继续，不能读取别的 Session 或任意宿主目录。`anchor_edit` 整体替换已有文件前先读摘要，或使用有匹配次数检查的文本替换。
- 下载、git、Cargo、依赖准备和候选 MCP 都按本 Turn 冻结的任务联网权限执行。用户明确禁止联网优先，候选的 `network:true` 不能给自己授权。没有网络时复用本地包与工具链，缺少依赖如实说明。
- 你不能自己把插件装进 Library：安装必须由人在原生表单里点确认。你也不能给自己增加权限、工具或插件绑定，不能修改或删除定义你自己的那张图，不能安装为自己提供指令的 Plugin——这些都要操作员在操作员接口里做。
- 凭证只以环境变量**名字**出现。不要把任何 token、API key、凭据写进插件目录、对话或命令参数。

## 目录契约

一个 Plugin 是「一个目录 + `plugin.json`」：

```text
<插件目录>/
├── plugin.json          # 必需；至少要有 name
├── skills/<name>/SKILL.md
├── .mcp.json            # 可选，等价于 plugin.json 里的 mcpServers
└── bin/…                # 入口程序或脚本，推荐用相对路径引用
```

`plugin.json` 里 `mcpServers` 的字段是**白名单**，出现未支持的字段会直接拒绝整包：

- stdio（有 `command`）：`type`、`enabled`、`startup_timeout_sec`、`tool_timeout_sec`、`command`、`args`、`cwd`、`env`、`env_vars`、`optional_env_vars`
- HTTP：`type`、`enabled`、`startup_timeout_sec`、`tool_timeout_sec`、`url`、`headers`、`http_headers`、`env_http_headers`、`bearer_token_env_var`、`auth`、`oauth_resource`、`oauth_client_id`、`oauth_scope`

规则：

- 任务工具需要联网时，在清单**顶层**声明 `"network": true`。HTTP MCP 的联网需求由宿主自动识别；stdio 业务客户端必须声明。挂载该 Plugin 后会依赖启用已安装的 `network` Plugin，但不会覆盖用户明确禁止联网。`network` 不是 `mcpServers` 的字段，也不是 Python 包依赖；没有启用者的任务工具在无网沙箱中运行。

- **未列出的字段会被拒绝**（例如上游清单里的 `dependencies`、`requirements`、`hooks`、`commands`、`agents`）。适配的主要工作之一就是把清单收敛到这个白名单。
- 凭证只能引用环境变量名：`env_vars` / `optional_env_vars`（stdio）、`bearer_token_env_var`、`env_http_headers`（HTTP）。`env` 和 `headers` 里写死值会随包分发，审查会标记它。
- `auth: "oauth"` 不能与静态 `Authorization`（`bearer_token_env_var`/`headers`）同时出现，会被拒绝。
- `optional_env_vars` 缺失时该 server **不会被启动**：想让它一定运行就不要用这个字段。
- 入口优先写相对路径（如 `bin/run`）。绝对路径必须属于宿主授权的工具环境。Python 第三方包写在清单顶层 `dependencies.python`，需要视图解释器时使用 `${ANCHOR_PLUGIN_VIEW}/bin/python`；调用 `plugin_prepare` 和 `plugin_test` 让宿主按授权来源处理，不修改宿主 Python 环境。Rust 使用部署共享 Cargo/rustup，按项目 `Cargo.toml`/`Cargo.lock` 构建；不要创建另一套工具链管理器。
- 符号链接、`.git` 不会被安装；目录里只放真的目录和普通文件。包内不要出现 `.env` 或其他凭证文件。

## 标准流程

1. **核查并获取**：先用 `plugin_list`、`plugin_read` 查看相关已安装能力、源码资源、依赖和工具，决定复用或开发。用户提供仓库或 HTTP 来源时实际获取到自己的工作区；保留仓库 commit 或下载内容摘要，不用移动的分支名充当最终版本。
2. **读懂**：读上游清单、入口和它真正要执行的东西；先判断这是 stdio 还是 HTTP，需要哪些环境变量。
3. **适配**：把清单收敛到上面的白名单；入口改成相对路径或登记过的工具环境；把凭证改成环境变量引用；不要顺手改上游代码而不记录。
4. **准备与自测**：对同一候选调用 `plugin_prepare`，记录候选摘要和实际锁定依赖；再用 `plugin_test` 真实启动 MCP、列工具，并传 `server`、`tool`、`arguments` 调用与需求相关的业务工具。仅列工具是协议发现，`business_tested:false` 不证明业务正确。候选测试不获取宿主凭据；需要真实凭据的部分留待安装、授权和挂载后验证。修改候选后重新准备/测试新摘要，失败、超时和取消不能记成通过。
5. **整理产物**：最终目录必须自包含。编译出的二进制必须同时带上可读源码和构建说明（这是审查规则：没有源码的二进制，人无法审查，也就不该被激活）。
6. **构建与依赖**：Python 写 `"dependencies": {"python": ["包==版本"]}`，候选准备使用 Session 私有视图，正式安装复用共享存储但生成独立正式视图。Rust 先查看 `plugin_prepare.rust_development` 披露的已装工具链；优先 `cargo build --locked`，指定版本用 `cargo +<已安装版本> build --locked`。只有项目确实要求新版本、宿主已配置共享 Rustup 且 Turn 允许联网时，才直接执行 `rustup toolchain install <版本>`。不要改共享 default、self-update 或写自造 linker/环境变量去绕过沙箱；构建命令可按实际需要传 `timeout_seconds`，最高 600 秒。把稳定入口放在候选 `bin/`，保留源码、锁文件与构建说明，不让正式入口引用开发工作区。旧 `environment` / `anchor.environment` 机制已删除。
7. **交审查**：先用 `plugin_review`（宿主工具，只传工作区内的相对目录）看一眼指纹、将执行的内容和告警，不满意就回去改；满意后调用 `plugin_install` 申请安装。`plugin_install` 只接受 `directory`（和可选的 `id`）——**确认字段不能由你提供**，它会把待批内容弹给操作员，操作员点确认才会真的装进库。取消、忽略、超时都算不批；同一指纹已装过会直接告诉你「已是最新」。缺少源码的编译产物默认装不进去，除非操作员额外勾一次「接受无法阅读的产物」，所以尽量把源码一起交出去。操作员也可以自己用 `anchor-library --root <Library> install --directory <候选目录> --id <id> --review` 看同一份审查单，再执行安装。

## 交给人的审查单里要有什么

- 来源：仓库地址 + 固定 commit（不是分支名）。
- 候选目录的内容指纹与文件清单。
- 相对上游改了什么（逐条，能对得上）。
- 将来会被执行的东西：每个 server 的传输方式、命令与参数、或 URL 与授权方式。
- 需要哪些环境变量**名字**（值不出现）。
- 依赖锁文件；编译产物则要有源码与构建命令，保证以后能离线重建。
- 实际被测摘要、依赖/工具链和业务调用结果；安装确认、已安装、已挂载和下一 Turn 实际可用分别核查。取消或恢复时先读取保存的文件和工具观察，再继续，不盲目重放不确定结果。

## 明确不要做

- 不要在运行期联网安装依赖、不要调用包管理器去改宿主的 Python/Node 环境。
- 不要为了让包「能跑」而放宽权限、去掉沙箱挂载或把凭证写进包。
- 不要把「没验证过」说成可用。自测通过和「人已批准激活」是两个不同状态，分别如实报告。
