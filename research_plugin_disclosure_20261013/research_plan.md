# 大量 Plugin 下的渐进式披露研究计划

研究日期：2026-10-13。

## 问题与交付

研究 Goose 和其他 Agent 产品在挂载大量 Plugin、Skill、MCP 工具时如何控制上下文、发现相关能力、取得完整调用参数，并在压缩和接续后继续可靠工作。对照 Anchor 当前代码提出复用现有框架和契约的方案；本任务不修改 Runtime、配置或部署。

交付：来源可追溯的研究报告、产品方案和 Anchor 当前实现差距。区分公开文档声明、固定版本源码事实、上游新功能、官方测量与本地实测；没有实际执行的机制不能写成已验收。

## 独立研究包

1. Goose：固定 v1.53.0 的 Top Of Mind、原生 Skills、Extension Manager、Code Mode、工具清单、压缩与 ACP；再核查当前上游的相关变更。确认在 Anchor 的 lean 构建和单一聚合 MCP 入口下哪些可直接复用，哪些需要接线或编译变化。输出 findings_goose.md。
2. Codex / OpenAI：Skill 元数据与正文的渐进式披露、MCP / Apps 工具搜索或延迟加载、大规模工具集合与上下文恢复。优先实际获取官方文档，必要时对照官方开源代码，标明证据层级及访问失败。输出 findings_codex.md。
3. Claude / Anthropic：Skills、MCP Tool Search、Tool Search API、工具引用和 Code Execution；如果有直接相关标准，核查其边界。重点提取大量工具时的阈值、检索召回、schema 加载、授权及恢复策略。输出 findings_claude.md。

每包以 3–5 次定向搜索为限；本环境没有 web_search / fetch_url 工具时，用 shell HTTP 客户端实际获取官方页面和公开源码，不把不可访问页面的搜索摘要当作已读证据。

## 主轨核查与综合

- 核查 Anchor 的 Library 定义、Session / Turn 挂载、Node / Pilot 工具装配、现有工具披露包装器及读取入口。
- 比较 Skill 指令披露和工具 schema 披露，区分模型上下文成本、宿主清单成本和 MCP 进程启动成本。
- 以唯一事实来源和 Host 授权为边界：只检索授权集合，发现不改变挂载，不维护第二份能力事实，不新增 Agent loop、记忆或恢复引擎。
- 评估几十 / 数百 / 数千 Plugin 和工具时是否需要分层目录、关键词或语义检索、完整 schema 获取和按需连接；说明召回和运行成功不能被无条件保证。
- 给出优先复用的实现方向及必要验收场景，不为方案讨论启动业务 Graph 或真实外部业务调用。
