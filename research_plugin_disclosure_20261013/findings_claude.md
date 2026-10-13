# Claude / Anthropic 与相关标准的研究结果

获取日期：2026-10-13（北京时间）。由主 Agent 研究。所有下列事实来自实际获取的第一方正文；原始内容、可读文本、URL、访问时间和 SHA256 位于 `sources/claude_*`、`sources/mcp_tools_spec.*` 和 `sources/skills_spec.*`。

## 来源和访问边界

1. Claude Code MCP 文档：https://code.claude.com/docs/en/mcp.md 。HTTP 200，119,468 B。
2. Claude Code Skills 文档：https://code.claude.com/docs/en/skills.md 。HTTP 200，107,130 B。
3. Anthropic，Advanced tool use，2025-11-24：https://www.anthropic.com/engineering/advanced-tool-use 。HTTP 200。
4. Anthropic，Code execution with MCP，2025-11-04：https://www.anthropic.com/engineering/code-execution-with-mcp 。HTTP 200。
5. Agent Skills 规范：https://agentskills.io/specification.md 。HTTP 200。
6. MCP 2025-11-25 tools 规范：https://modelcontextprotocol.io/specification/2025-11-25/server/tools.md 。HTTP 200。

Tool Search API 文档的 HTML 和 `.md` 两条请求都被重定向到 `https://claude.com/app-unavailable-in-region`。虽然 HTTP 200，实际不是 API 文档，不能用来补签 API 行为；相关 API 机制仅引用已获取的官方工程文章，产品行为另引用 Claude Code 文档。本研究没有执行 Claude / Anthropic API 调用或本地性能测试。

## Skills：目录、指令和资源是三个层级

官方 Agent Skills 规范的原文：

> Metadata (~100 tokens): The name and description fields are loaded at startup for all skills.
> Instructions (< 5000 tokens recommended): The full SKILL.md body is loaded when the skill is activated.
> Resources (as needed): Files ... are loaded only when required.

这是作者格式与通常加载策略，不是“任意多条目录仍然保持常数成本”的协议保证。1000 个 Skill 即使每条只有约 100 token，也会约占 100,000 token；此数是按规范举例的算术推导，不是实测。

Claude Code 当前文档进一步说明：普通会话先加载 Skill description，仅在调用时加载全文；带预加载 Skills 的子 Agent 会在启动时加载全文，不能混为同一策略。

当前大目录策略有明确限额：

> The listing always contains every skill name ... Claude Code drops some descriptions to fit the listing's character budget ... The budget scales at 1% of the model's context window.

超预算时先丢弃最少使用 Skills 的 description；可以设 `skillListingBudgetFraction` / `SLASH_COMMAND_TOOL_CHAR_BUDGET`，或把低优先项设为 name-only。单条 description + when_to_use 默认最多 1,536 字符，且可配置。保留所有名字仍有 O(N) 成本；删除 description 也会减少任务匹配关键词。官方明确把这作为“Skill 不触发”的原因，并建议目录诊断和描述评测，所以不能称此机制无条件保证全部 Skill 可发现。

## 工具：完整 schema 延迟加载，不只是省略正文

Claude Code 当前 MCP 文档：

> Only tool names and server instructions load at session start.
> Tool search is enabled by default: MCP tools are deferred and discovered on demand.

准确的当前配置区别：未设 `ENABLE_TOOL_SEARCH` 时默认延迟；`auto` 才是“工具定义到上下文窗口 10% 时切到延迟”的阈值模式；`auto:N` 自定义百分比。不要把历史上的自动阈值当成当前默认。

可用 `alwaysLoad` 保留少量高频工具。server instructions 用于让模型知道什么时候搜索这类工具；每个 server instructions 和 tool description 默认截到 2,048 字符。发现依赖描述质量，而不是只有工具名字。

官方工程文章解释 API 机制：所有工具定义交给 API，但 `defer_loading: true` 的工具不进模型初始上下文。检索返回 tool references，再展开完整 definitions。内置检索有 regex、BM25，也允许自定义检索，包括 embeddings。建议只常驻 3–5 个高频工具。这里的 3–5 是产品建议，不是 Anchor 的硬阈值。

可移植性限制：当前 Claude Code 文档要求支持 `tool_reference` 的模型；非第一方代理通常回退提前加载。部分部署端直接拒绝，配置开关不能覆盖。这不能直接视为 Goose + DeepSeek / 任意 OpenAI-compatible Provider 可用的功能。

官方文章的“85% token 降低”、Opus 4 49%→74%、Opus 4.5 79.5%→88.1% 均为 Anthropic 的特定内部实验；只说明延迟工具机制可能改善成本和选择准确率，不是 Anchor 收益测量，更不是每次成功保证。

## 启动：远程 MCP 工具发现缓存与首次调用连接

当前 Claude Code MCP 文档有另一项独立优化：HTTP / SSE MCP 的 tools inventory 可以来自前次会话的发现缓存，显示 `cached ... connects on first use`；实际调用该 server 的工具时才连接。要求 v2.1.221+。当前文档还说明发现缓存默认关闭，除非账号渐进发布已启用，也可由 `MCP_DISCOVERY_CACHE=1` 打开；不能说所有 Claude Code 用户默认都懒连接。

其他 server 默认在后台连接，ToolSearch 可以等待需要的 server；配置也允许启动时等待。Tools Search 与异步连接/缓存解决的是不同成本。

失败、缺凭据或仍在连接的 server 会影响实际可用性，工具搜索结果应报告相关状态，而不是把“没有匹配”直接解读为“没有安装”。当前 Claude Code 文档也说明是否把连接失败告诉模型受 tool search 配置影响。

缓存不是授权：禁用 server、清除认证、重连等会丢弃或更新缓存。对 Anchor 而言，即使采用发现缓存，也必须按当前授权过滤，在首次连接和调用前重新核查配置、凭据和真正 inventory。应是可重建观察，不能成为第二份 Plugin 定义。

## 生命周期：已加载内容会被压缩，权限另行校验

Claude Code 的 Skill 原文进入会话后跨 Turn 保留；完全相同内容再次调用会给已加载提示，内容不同则再次加入。压缩后保留最近一次调用的每个 Skill 前 5,000 token，总预算 25,000 token，优先最近调用；更多 Skills 时旧内容可能整条丢失。官方明确建议需要时再次调用。

`allowed-tools` 的权限授权只对相应 Turn 有效，即使指令仍在历史里。这证明“模型看过指令”与“当前有调用权限”应分开；Anchor 不应引入持久的已加载标志当作权限事实。

## Code execution：同时优化工具定义和结果，但有额外执行边界

另一篇工程文章提出把 MCP 工具生成为 TypeScript 文件/API：Agent 浏览目录，只读所需的函数接口；可添加 `search_tools`，按 name、description、full schema 不同细节等级获取。中间数据留在执行环境，模型只读必要输出。

文章展示 150,000→2,000 token（98.7%）的特定例子。这是工具表示与工作流设计的示例，不是通用性能保证。程序化调用与 schema 延迟搜索是互补机制；引入新执行器前应先验证沙箱、授权、取消、每次实际调用的观察与副作用恢复。Anchor 已有 authoring sandbox，但它并不自动等同于工具调用用的 Code Mode。

## MCP 分页不是模型渐进披露

MCP tools/list 本身支持 cursor 分页、inputSchema、listChanged 通知及 tools/call。客户端完全可以把所有页拉完，再把所有 schema 发给模型。因此使用 MCP 或 list 分页不能单独证明模型上下文已经按需加载。

## 对 Anchor 的直接启示

- Skill 指令与工具 schema 都要独立分层；tom 可以提醒使用发现入口，但不能承担无限目录。
- 始终可用的查询入口、精确 Plugin / Skill / Tool ID 查找和分页保证可达性；自然语言检索只返回有限相关项，description 应有明确适用条件。
- 工具详情必须包含完整参数 schema 和必要示例，避免只搜索到名字后猜参数。
- 一般 Provider 上可在 Anchor 的已有 ToolPort 披露边界传回 schema，并继续通过授权调用端口执行；不要假定支持 Anthropic 的 tool_reference。
- 数百 server 的启动开销需要单独评估。模型 schema 延迟加载不会自动延迟 MCP 进程启动。
- 目录、检索和调用都以同一授权集合为边界，模型的发现行为不自动安装或挂载。
- 压缩后的重读交给 Agent 与 Goose 原生会话，不能照搬 Claude 的另一套恢复或记忆系统。
