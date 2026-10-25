# Anchor

<p align="center">
  <strong>把大模型的判断力接到真实业务上，而不把边界交给概率。</strong>
</p>

<p align="center">
  <a href="LICENSE">MIT</a> ·
  Rust Host + 共享 Runtime Kernel ·
  <a href="docs/architecture.md">Goose ACP</a> ·
  React / TypeScript WebUI
</p>

Anchor 是一个**个人 agent 平台**：作者用自己的工作和生活负载在跑它，用它承接反复占用时间和注意力的事，再用它自己的执行记录改进它自己。

它不是给陌生人的 SaaS，也不是又一个拖图工具。它回答的是更窄的问题：

> 大模型是概率系统。要让它在真实业务上长期工作，需要什么样的条件？

## 一个判断

今天的 agent 系统几乎都在优化同一件事：让模型更会做事。但把 agent 接进真实业务，卡住人的是另一类问题——它做了什么、花了多少、为什么失败、能不能接着做、这次和上次的差别是变好还是只是变了。

这些问题回答不了，不是因为模型不够强，而是因为**执行过程从来没有被记成数据**。聊天记录不是记录：它没有结构、没有成本、没有权限边界、没有可恢复的游标，也没法比较两次执行。

Anchor 的选择是：**先把执行变成事实，再谈智能。**

## 押注：给条件，不捏雪花

把业务直接扔给模型，得到的是不可控的概率。所以 Anchor 划了一条线：

- **交给代码**：身份、权限、schema、路由、持久化、机械门禁、幂等、冻结。这些错了不会有人提醒你，模型再强也补不回来。
- **交给模型**：怎么拆、读到什么、怎么综合、返工几次、结论是什么。这些钉死了，模型变强也没有空间。

这条线由 Graph 落地：**AgentNode** 承担需要理解、搜索、取舍和综合的工作；**OpNode** 承担需要代码严格控制的步骤——命令、Graph 调用、扇出与汇合、获授权的宿主操作。两者在同一条边上协作，而不是让模型独自决定整条流程。

由此有一个可检验的标准：**把模型换成更强的一代，这个平台应该删代码，而不是改代码。** 如果每次换模型都要重新适配拓扑和门禁，说明约束渗进了判断层——那是在用编排补偿模型，永远追不上。

## 四个设计决定

**执行是一条记录，不是一段日志。** 每笔提交经同一个 `Kernel::submit` 落成同一种历史记录；时间、成本、身份由宿主盖章，内核只记录、不据此裁决。Run 开始即冻结 Graph、输入和资源授权。可观察、可恢复、可度量都是这一个决定的推论，不是三个独立功能——UI、摘要和模型上下文都只是事实的投影。

**能力是文件，所以能被交付，也能被改写。** Graph 是 JSON，能 diff、能在画布上编辑、能随每条 Run 冻结快照、能打成不含源码的包交给别的机器。Plugin 是显式资产；节点声明了工具不等于拿到权限，授权由 Host 与 Sandbox 执行——模型被注入、图被改坏，边界还在。

**边界在执行层，不在提示词里。** 每节点稳定的 workspace（`<run>/nodes/<node-hash>`）+ 不可变产物谱系 + Bubblewrap 沙箱。外部副作用结果未知时，先保存观察，再让 Agent 核查现场，然后继续、补偿或提问——不自动重放。

**因此它能改进自己。** 执行记录是统一口径，Graph 和 Plugin 是文件资产，于是「读记录 → 提出改动 → 改图 → 跑一次 → 比较 → 回写」是一条闭环，不需要外挂任何东西。这是 Anchor 最不像其他工具的地方：**它的产物就是它自己的输入。**

## 它不是什么

说清边界比列功能更有用：

- **不是多租户云平台。** 单进程、本机、一个安装身份，面向 Linux x86_64；没有 HA、滚动升级和集中告警。
- **不追求第一小时的顺滑。** 要编译 Rust、装 Bubblewrap、用固定版 Goose，上手成本是真的。
- **不承诺外部副作用的 exactly-once。** 它承诺的是：未知的操作不会被当成已知重放，事后能查清发生了什么。
- **隔离强度是短板。** Bubblewrap 是沙箱方案里较轻的一端，Host 与图执行同进程。它在记账上很严，在隔离上一般。

如果你的 agent 只是跑在自己电脑上、结果发条消息就算完，n8n、Dify、扣子更省事，那是更好的答案。Anchor 值得用的场景很窄：**能力要装到不能上云的机器上、有外部副作用、要一直活着，而且你希望它靠测量而不是感觉变好。**

## 现在能做什么

| | |
| --- | --- |
| 编排 | 条件路由、反馈回路、内联子图、独立 Graph Call、`wait/detach`、配对 `fanout/join`；可在画布编辑，也可由 Pilot 生成 |
| 运行 | 路径、节点对话、工具轨迹、工作区、Artifact；暂停、停止、继续、受保护删除 |
| 持续协作 | Session/Turn、SSE 重连、必要提问、原生会话续聊与压缩 |
| 能力插拔 | Plugin = Skill + 资源 + stdio/HTTP MCP + 凭据引用；Library 安装后由 Graph 显式引用 |
| 触发与渠道 | 手动、计划、Webhook、Responses、企业微信；一张常驻助手 Graph 对应一个机器人，每会话一个常驻 Run，重启原地接续 |
| 自我改进 | `rsi` Graph 采集执行记录与代码，并行专项审查，机械门禁校验证据与提案，产出带验收和回滚条件的变更提议 |

首次启动提供 7 张示例 Graph 和 5 个官方 Plugin。建议先跑不需要模型的 `call-report`，再配置模型尝试 `revise-loop` 和 `plugin-research`；清单见[上手示例](examples/README.md)。

## 快速开始

```sh
git clone https://github.com/manstein-lzn/Anchor.git
cd Anchor
./scripts/dev.sh start     # 首次编译 Host、WebUI 与固定版 Goose，需要几分钟
```

打开 <http://127.0.0.1:5173>，在「设置 → 模型服务」填服务地址、协议、API key 和模型名，即可与 Pilot 对话。无需 `.env`，凭据存在 Host 私有存储里。`scripts/dev.sh` 使用独立的 `.local/rust` 数据根，不会接管仓库里已有的 Graph 和运行记录。

需要 Linux + Rust stable + Node.js 20.19+/22.12+ + Git + Bubblewrap + slirp4netns；细分要求见[使用指南](docs/usage.md#安装与本地启动)。

## 当前状态

维护路径是 Rust Host + 共享 Runtime Kernel + Goose ACP + React/TypeScript WebUI，默认面向本机单进程部署。核心运行链有确定性本地回归（小型 fixture Graph + 确定性本地 Provider，实际经过 Host、GraphRunner、Goose ACP 与授权 MCP）；真实模型、真实外部服务、目标机发行和旧数据迁移分别验收，结论与未关闭事项见[验收台账](docs/pilot-development-plan.md)。本地回归通过不等于生产流量已经切换。

## 文档

| 想了解 | 从这里开始 |
| --- | --- |
| 安装、Graph、Run、Session | [使用指南](docs/usage.md) |
| 当前实现与事实所有者 | [当前架构](docs/architecture.md) |
| 产品边界、执行不变量、设计原则 | [产品与系统架构](docs/product-architecture.md) |
| 为什么值得投入：执行记录与自我改进 | [Harness 数据中枢](docs/harness-data-hub.md) |
| 自我改进的具体做法 | [RSI](docs/rsi.md) |
| Plugin、Skill、MCP | [Plugin 指南](docs/plugins.md) |
| 企业微信助手 | [企业微信指南](docs/wecom-assistant.md) |
| 验收结论与待办 | [验收台账](docs/pilot-development-plan.md) |

## 开发

```sh
./scripts/test.sh                        # cargo test，测试临时文件放 tmpfs
cargo +stable clippy --manifest-path rust/Cargo.toml --workspace --all-targets --locked -- -D warnings
npm --prefix apps/web test && npm --prefix apps/web run build
```

开始改动前请读 [AGENTS.md](AGENTS.md)。Kernel 契约冻结，业务代码只能通过 `application/run_access` 门面读写 Run。

## 许可证

[MIT](LICENSE)
