# Goose 的 Plugin / Skill / MCP 渐进披露研究

研究日期：2026-10-13。仅阅读官方资料和源码，没有修改 Anchor Runtime、部署、配置或台账，没有运行新的 Provider 验收。

## 结论

Goose 确实有针对同类问题的成熟机制，但 **tom 不是 Goose 的 Plugin 发现或工具检索系统**。固定 v1.53.0 中，Skill 正文靠原生 `load_skill` 按需加载；大量 MCP 工具靠可选 **Code Mode** 隐藏常规 schema 并逐步取得函数定义；Extension Manager 则动态启用/停用整个 Goose extension。这三者解决的成本和授权粒度不同。

Anchor 可以直接复用 Goose 的 Agent loop、上下文压缩、ACP/MCP 和 tom 的短提醒。不能据此声称当前 Anchor 的 Plugin 已接入原生 Skills 或 Code Mode。其 lean 二进制未编译 `code-mode`；原生 Skills 的文件发现根和 Plugin 状态不等于 Anchor 的授权集合；Extension Manager 看不到单一 Anchor MCP 内部的各 Plugin。

对于数百/数千 Plugin，不建议把所有 Skill 描述每轮塞进 tom。Native Skills 的平铺摘要和 Code Mode 默认 `list_functions` 也不是无条件可扩展的检索方案；都仍可能输出 O(N) 目录。短常驻提醒 + 授权能力检索 + 按具体 Skill / 工具取得正文和 schema，才同时控制可发现性和上下文成本。

## 证据范围与来源

1. 当前官方站点是 `https://goose-docs.ai/`。实际获取并保存全文和纯文本：
   - [Agent Skills](https://goose-docs.ai/docs/guides/context-engineering/using-skills/)：`goose_live_skills.{html,txt}`。
   - [Code Mode](https://goose-docs.ai/docs/guides/managing-tools/code-mode/)：`goose_live_code_mode.{html,txt}`。
   - [Extension Manager](https://goose-docs.ai/docs/mcp/extension-manager-mcp/)：`goose_live_extension_manager.{html,txt}`。
2. 固定 v1.53.0 关键源码通过官方 GitHub raw / contents API 实际取得，与本地源码逐字节一致；检查见 `goose_official_source_checks.json`。包括 Skills、Extension Manager、tool list、推理工具过滤和文档。`code_execution.rs` 的官方 raw SHA256 为 `fd5ace5986d6dbb0c2398737cb042238f6e227b7779d10d8f2fe653c85017d3b`；本地相同。
3. 当前上游明确固定到 commit [69de89c50cc01d4e0187d6645bf3618729873f74](https://github.com/aaif-goose/goose/commit/69de89c50cc01d4e0187d6645bf3618729873f74)，提交时间 `2026-10-12T20:14:04Z`；commit / tree API 及四份关键源码已保存，差异见 `goose_current_*.diff`。不把主干实现说成 Anchor 当前能力。
4. 部分 raw 请求超时或网络不可达，旧 `block.github.io/goose` 文档地址返回 404；随后使用 GitHub contents API 和新官方站点取到所需正文。未使用搜索摘要代替原文。

## 固定版机制

### 1. tom / MOIM：持续短提醒，不负责目录检索

官方源码：[tom.rs](https://github.com/aaif-goose/goose/blob/v1.53.0/crates/goose/src/agents/platform_extensions/tom.rs)、[moim.rs](https://github.com/aaif-goose/goose/blob/v1.53.0/crates/goose/src/agents/moim.rs)。本地摘录 `goose_local_tom.txt`、`goose_local_moim.txt`；两文件此前已由主轨逐字节核对官方 fixed raw。

- `tom` 不提供工具，`get_moim` 读取 `GOOSE_MOIM_MESSAGE_TEXT` 和 `GOOSE_MOIM_MESSAGE_FILE`；每个来源各限 **65,536 bytes**，截断没有提供分页或检索。
- MOIM 是持久保存的 agent-only user message。源码明确：`composed once per turn, persisted ... never moved or edited afterwards`；旧块留在历史，只有最新块是当前事实。不能当成不消耗上下文的持久系统目录。
- Goose 识别到模型上下文限额 **< 32,000 tokens** 时跳过 MOIM；限额未知时不会因此跳过。因此仅靠 tom 让 Agent 知道如何发现 Plugin，会留下小上下文模型和关闭 tom 时的入口缺口。
- 适合放“能力存在、如何搜索和读取、当前权限/目录变更”的短提醒，不适合不设预算地放全量 Plugin / Skill 正文或描述。

### 2. 原生 Skills：摘要 → 正文 → supporting file

官方原文：

> When a session starts, goose adds discovered skill names and descriptions to its instructions.

> During the session, goose can load a skill's full instructions when [the request matches or the user explicitly asks].

源码：[SkillsClient](https://github.com/aaif-goose/goose/blob/v1.53.0/crates/goose/src/skills/client.rs#L245)、[skills/mod.rs](https://github.com/aaif-goose/goose/blob/v1.53.0/crates/goose/src/skills/mod.rs#L356)。

- `get_instructions` 列出所有发现的 Skill `name - description`；没有分页、查询、top-k 或总 token 预算。
- 对模型只提供一个 `load_skill(name, args?)`。`name="skill-name"` 加载正文；`name="skill-name/path"` 加载 supporting file。supporting file 读取有 confinement、编码和响应长度限制；无需开 Developer 文件工具才能通过 `load_skill` 读取。
- 这是**对模型的按需披露**，不是宿主磁盘 IO 的完全延迟读取：`discover_skills` 扫目录，读取完整 SKILL.md，解析 frontmatter/body，并枚举 supporting files。大量目录仍有宿主扫描成本。
- 实际根包括工作区的 `.agents/skills`、`.goose/skills`、`.claude/skills`，用户 skills 路径、Goose config 路径和 Goose 已发现的 Plugin skill roots。**不自动扫描 Anchor 的 `/plugins/<id>`**。
- ACP `session/new/load` 没有发现接受“本 Session 的授权 Skill roots”的字段。`additional_source_roots` 存在于嵌入式 `AcpServerFactoryConfig` / `GooseAcpAgentOptions`，用于 `sources/list` 和 source CRUD；`load_skill` 调用的 `discover_skills` 不接收它。不能把这个配置当成现成的授权 Skill 注入接口。
- Goose Plugin 发现还会扫描 project/user 插件路径、读取并写入 Goose `plugins` enabled 配置，并可自动更新 Goose 安装的插件。Open Plugins 安装器会把 Skill name 改写为 `plugin-name:skill-name`。直接把 Anchor Library 映射为 Goose 原生 Plugin 生态会出现第二套 Plugin 状态/内容管理，需要认真适配。
- 更精确地说，Anchor 当前传入 `_meta.enabledExtensions`，固定版 ACP 的 `initial_session_extensions` 不会走 Goose Plugin MCP 自动发现分支。不能说“只放入目录就必然在当前 Anchor 自动启动未授权 MCP”。但省略显式 extensions 的其他入口确实有该分支。
- 固定版还有实验性 `GOOSE_STATE_MACHINE` 路径，它有内置 SkillOperation，独立添加 `load_skill` 和 Skills 提示。默认关闭；Anchor 当前没有开启。以后切换此路径时需重新核查授权，不能只依赖删除 `skills` extension。

### 3. Code Mode：把工具 schema 隐藏为少量发现/调用工具

官方原文：

> Tools from enabled extensions are discovered on-demand and loaded into context as needed.

> Every LLM call includes the 3 meta-tool definitions, plus any tool definitions previously discovered in the session.

> Code Mode only supports text content from tool results. Images, binary data, and other content types are ignored.

固定源码：[code_execution.rs](https://github.com/aaif-goose/goose/blob/v1.53.0/crates/goose/src/agents/platform_extensions/code_execution.rs)、[prepare_inference_tools](https://github.com/aaif-goose/goose/blob/v1.53.0/crates/goose/src/agents/reply_parts.rs#L245)。

- `CODE_MODE_TOOL_DISCLOSURE=catalog`（默认）：`list_functions` → `get_function_details` → `execute_typescript`。普通 extension 的工具 schema 从推理 tool list 移除；first-class platform tools 和特定 resource tool 例外保留。因此“模型永远仅看到 3 个工具”不是源码的无条件保证。
- `list_functions` 没有 query/cursor 参数；Goose 直接返回 PCTX 的全部函数目录。它把 schema 初始成本降低，但非常多工具时平铺目录仍可能很大。
- `filesystem`：`execute_bash` 搜索/读取 PCTX virtual filesystem 中函数签名和类型，再 `execute_typescript`；这是 PCTX 的虚拟工具定义文件，不应与 Anchor 的宿主 shell 工作区混淆。相较全量 `list_functions`，这条路径更接近大规模目录按需检索，但 PCTX 0.5.0 内部搜索、输出限制和隔离的具体效果未在本地跑通。
- `sidecar` **不做渐进式搜索**，全部工具仍保留，主要提供调用编排。源码明确 `in sidecar style there is no progressive search`。不应把它作为工具过多的解决方案。
- Host/Goose 仍收集完整 tool list。Code Mode 的 callback configs 来自 `get_prefixed_tools_excluding`，包含工具描述和 input schema；Goose ExtensionManager 在缓存 miss 时对已连接 extension 调 `list_tools` 并读完 cursor 分页。因此这是**模型 schema 披露延迟**，不是 MCP server 首次使用才连接。
- 固定版回调仍走 ExtensionManager 的 `dispatch_tool_call`，不会自行执行 Anchor Plugin 命令或建立另一条调用链；Host MCP 仍可执行自己的授权校验。
- 命名先按 `__` 拆 namespace，再保留剩余工具名。Anchor 聚合 MCP 的所有工具会落在同一 `anchor` namespace，其内部 `plugin-tool` 名称能否都被 PCTX callback 注册为合法可调用函数，需要验证，不能仅凭支持 MCP 就承诺兼容。
- 回调返回把 assistant-visible Text 合并并尝试解析 JSON，丢弃非文本、structuredContent 和 `_meta`；这一转换没有读取 `CallToolResult.is_error`。故业务失败可能只成为字符串/JSON 值，脚本继续执行；不能把正常 MCP 完整回执语义视为自动保留。
- callback 克隆同一个 ToolCallContext，复用外层调用身份；嵌套调用的 Anchor trace、用户问题/elicitation、取消、局部成功/失败、多媒体输出仍须验收。现成代码已有超时和取消令牌传递，不代表 Anchor 产品投影已验收。
- Anchor 构建脚本只有 `--no-default-features --features rustls-tls,online-model-meta`，没有 `code-mode`。该 feature 引入 `pctx_code_mode=0.5.0` 和 Deno 执行路径。是否能维持当前静态 musl 发行形态、体积及资源消耗须独立验证；不是改一个配置即可启用。

### 4. Extension Manager：动态加载整个 extension，粒度不匹配 Anchor Plugin

官方原文：

> For optimal performance, aim for 5 or fewer active extensions with a total of 50 or fewer tools.

这属于官方建议，不是强制上限或模型效果保证。

源码：[ext_manager.rs](https://github.com/aaif-goose/goose/blob/v1.53.0/crates/goose/src/agents/platform_extensions/ext_manager.rs)。

- `search_available_extensions` 没有查询参数，返回全量 enabled 名字和 disabled 名字/描述；不是语义检索。
- `manage_extensions(enable/disable, extension_name)` 从 Goose 全局配置取得 extension，并改变 Goose extension 集合；启用会启动相应连接，停用可降低活跃 schema 数量。
- Anchor 对 Goose 只暴露一个聚合 MCP `anchor`。这个管理器无法启用其中某个 Plugin，也不能从 Anchor Library 获得可用 Plugin 清单。
- 直接开放原生管理器，会让模型控制 Anchor MCP 的移除或 Goose 配置中额外 extension 的启动，与 Anchor 已有 mount/permission 事实冲突。应借鉴“授权不变、按需激活”的思想，继续让 Anchor 管理其内部资源，不能用 Goose enable 状态代替 Host 授权。

### 5. Compaction 与重新发现

固定 Goose 自动压缩默认阈值是 0.8；恢复、摘要和历史可见性由 Goose 原生负责。正文和已发现工具定义处于会话/tool result 内，可能经过压缩；压缩不应被当成在极多工具 schema 常驻时无限扩容的方案。

Anchor 应保留随时可用的稳定发现/读取入口，让 Agent 在摘要不够时重新取得定义；目录与读取结果应来自同一授权事实，不维护“永久已读所以永不再读”的 Anchor 记忆状态。

## 当前上游与固定版的区别

截至上述主干 commit：

- `skills/mod.rs` 与 v1.53.0 完全一致；目录还是全量摘要，发现根未增加 per-session authorized roots。
- SkillsClient 的 working directory 改从当前 ToolCallContext / get_instructions 参数获取，减少实例可变工作目录状态。
- Code Mode 改为 `ExtensionLease` 获取和调用工具，回调在 lease 上执行并处理 extension mutation；Extension Manager 也改为 lease 和显式 mutation。这是并发/调用事实的一致性演进。
- Code Mode 仍是 catalog/filesystem/sidecar；catalog 仍无 query/cursor；text-only 转换和 `output_schema: None` 仍在。
- 因此升级到主干并不能自动替 Anchor 完成授权能力检索、Plugin 根适配、失败回执和媒体投影。本次不建议以升级 Goose 代替补齐 Anchor 的接线。

## 对 Anchor 的方案建议

1. **tom 保持短**：常驻说明发现入口和当前能力集合的版本/状态；少量 Plugin 可给受预算约束的摘要，多量时明确“更多可通过查询取得”。不在 MOIM 中全量铺开 N 个 Skill。
2. **以 Anchor 授权集合生成发现视图**：检索挂载能力和 Library 管理目录分开；安装不等于可调用，查询不改变 mount/授权。目录只从 Library / 本次准备结果投影，不能建第二套 Plugin 状态。
3. **统一 Skill 级读取和精确 schema 查询**：借鉴 Goose `load_skill` 与 Code Mode 的“摘要、正文、资源/类型”分层；Pilot/AgentNode 共用稳定 Host MCP 契约。现有 Anchor 工具披露层是比无条件开启原生扩展更直接的接线位置。
4. **Code Mode 作为可选执行优化，不作为首轮必需前提**：若后续需要批处理和本地处理大量中间结果，再做可运行验证。先解决检索与 schema/回执契约，就能保持 lean Goose 和正常 MCP 的媒体、失败、授权语义。
5. **把连接成本独立安排**：上下文问题可以先用工具披露解决；若确有多 MCP 冷启动成本，再研究 Anchor 内部按 Plugin/endpoint 激活，不要把 schema 延迟加载宣称为已实现懒连接。

## 尚未验证

- 新的真实 Provider、大量 Plugin 实际选用召回和任务成功率；包括同名/近义描述、跨语言查询、多步跨 Plugin 工作和压缩后继续。
- PCTX 对 Anchor 工具名称、静态 musl 发行、MCP `is_error`、图片/resource、elicitation、trace、取消/超时的端到端兼容。
- Goose / Anchor 数千 Plugin 的实际磁盘扫描、MCP tool list 缓存、启动资源消耗。
- 没有发现 Goose 官方针对“数千 Plugin 在 Anchor 的聚合 MCP 下”给出性能或成功率保证；不能把一般宣传或 5/50 建议当成此问题已被无条件解决。
