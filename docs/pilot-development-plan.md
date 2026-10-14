# 当前验收与待办

本页只维护当前结论、未关闭事项和证据入口，直接更新对应条目。当前行为见 [架构](architecture.md)，产品方向见 [产品架构](product-architecture.md)，操作见 [Pilot 开发指南](pilot-development.md)。

以下结论按既有分片验收归纳，证据基线截至 2026-10-13、提交 `c7a4635`，不代表该提交的全产品联合验收。旧 A 编号、命令、失败经过和二进制身份完整保存在 [历史台账](archive/development-2026-10-13/pilot-development-plan.md#验收矩阵与证据台账)，无需在开工时通读。

## 当前验收结论

| 能力 | 已验范围 | 证据入口 |
| --- | --- | --- |
| Rust Kernel、Host 与 Goose ACP | AgentNode/Pilot、原生历史、问答、恢复、取消和 compaction 有确定性及分片真实模型证据；旧执行器记录保持原身份 | 历史台账 A114–A140；[回归入口](runtime-contract-tests.md) |
| Pilot 获取、开发与交付 Plugin | Python/Rust 开发、自测、确认安装、挂载、正式调用和删除开发 Session 后调用通过；受控 HTTP 获取另有阶段证据 | 历史台账 A207/A212；不代表公共市场兼容 |
| 空业务安装初始化 | Python/Rust 冷初始化、私密凭据 fixture、Graph 运行和计划登记分别通过，绑定不同冻结包 | [A216 范围表](archive/development-2026-10-13/pilot-development-plan.md#pilot-初始化与开发依赖验收a216)；详细产物在原机器 `.local/pilot-bootstrap-acceptance/` |
| 私有 Python 开发环境 | 安装/升级/卸载、editable、隔离、项目锁、取消与旧视图兼容有定向证据；真实模型完成独立正式交付及删除现场后调用 | [A225 范围表](archive/development-2026-10-13/pilot-development-plan.md#开发环境自由度验收a225)、[可复跑验收材料](../research_development_environment_20261013/README.md) |
| Plugin 发现、读取与工具调用 | Pilot/Node 完整披露、授权调用和有界目录通过；持续摘要、挂载接续及压缩分别有真实/确定性证据 | [A222](../research_plugin_disclosure_20261013/acceptance/README.md)、[A224](../research_plugin_disclosure_20261013/capability-context/README.md) |
| Pilot 默认联网 | 网页与 Responses 新会话默认挂载、用户禁止和旧会话边界通过；隔离 Host 实际安装 requests 并在断网后 import | 历史台账 A204/A206/A228；A228 原始产物在原机器 `.local/pilot-network-default-20261013/` |
| Web 问答与 API key | 问答按对话顺序回看，错误 key/无效字符拒绝及正确 key 加载通过真实浏览器验收 | 历史台账 A221/A227 |
| Pilot 初始界面 | 以任务、工作流和工具扩展呈现入口，示例只填草稿、首次发送和失败重试保持幂等，插件默认收起；140 项 Web 单测与 19 项相关浏览器用例通过，8077 桌面/手机只读实测及上线前后会话、挂载与资源事实一致 | [初始页回归](../apps/web/e2e/pilot-start.spec.ts)、[会话回归](../apps/web/e2e/pilot.spec.ts)；本机证据 `.local/pilot-start-ui-20261014/deployment-check.json`，本轮未新增真实模型验收 |
| 插件界面 | Pilot/Graph/Run 共用卡片、搜索和详情，草稿/即时挂载/历史记录分别展示；140 项 Web 单测及 21 项相关浏览器用例通过，当前 8077 已更新并经只读实测确认既有会话与设置未变 | [跨面板回归](../apps/web/e2e/plugin-experience.spec.ts)、[原生挂载回归](../apps/web/e2e/session-plugins-native.spec.ts)；本机部署证据 `.local/plugin-ui-20261014/deployment-check.json`，外部 OAuth 未新增真实验收 |
| 渠道与常驻助手 | 企微私聊收发有历史部署证据；常驻同 Run、稳定 workspace、打断/恢复有确定性与隔离真实模型证据 | 历史台账 A143–A157；[渠道验收边界](wecom-assistant.md#docmost-和验收) |

## 正在收口

| 事项 | 当前状态 | 关闭条件 |
| --- | --- | --- |
| 当前开发部署的依赖准备配置 | A228 的开发 Host 未配置依赖准备器；隔离 Host 安装成功不能代替这里可用 | 在目标部署核查配置，并实际通过 Pilot 准备、安装和调用依赖 |
| MCP 按需连接 | A223 仅完成固定 Goose 原生接口实验，产品尚未实施 | 按 [短路线](goose-native-capability-reuse-plan.md) 验发现、首次连接、调用与回收及权限/取消边界 |

## 保留的验收边界

- **初始化与正式交付**：尚无同一最终发行包的空机器全流程联合验收；A216、A225、A228 各自的结果不能互相补签。计划登记不证明未来计划已执行。
- **Python 兼容**：A225 真实场景为零第三方依赖 editable 与合成正式 wheel；第三方构建后端/native 包、规模压力、额外长路径及跨 ABI/不同解释器构建未专项验收。仅解包 store 不能供私有 pip 恢复包，是已确定的兼容边界，见 [开发指南](pilot-development.md#旧项目兼容)。
- **披露规模与压缩**：1,000 Plugin/工具目录不代表 1,000 MCP server 启动；A224 Pilot 与最终 Node 绑定不同 Host。真实 Pilot 压缩后未重新调用工具，重读由确定性场景验证。懒连接与 Code Mode 未实施。
- **运行时组合**：更完整的 `call.session` 业务组合、多次/超大上下文、摘要 Provider 错误及提问/媒体组合未全部验收；Pilot 未直接暴露手动 `/compact`。固定 Goose ACP 未提供完成工具参数的逐 token 增量，不以普通文本冒充结构化完成，也不为此新增模型代理。
- **外部业务**：真实外部 OAuth、最终常驻路径的企微收发/媒体/vision、Docmost 和公共学术服务仍需按具体场景验收；已有私聊或本地 mock 结果不扩大为全部通过。
- **发行与生产切换**：目标发行版 systemd/动态库、旧数据迁移与回滚、标准业务 Graph 完整兼容仍未统一验收。开发部署 A226/A228 不等于生产切换，操作边界见 [切换指南](rust-production-cutover.md)。
- **未决定的范围**：Plugin 多条目升级事务/自动回滚、跨设备准备区和旧目录处置见 [依赖标准的开放问题](plugin-dependencies.md#开放问题)；长期 Copilot、研究应用、可见计划、附件/分支/导出等需求见 [产品架构](product-architecture.md)。它们不自动变成当前开发任务。

## 查证方式

按能力进入对应证据，只把实际命令与结果作为验证结论。历史验收绑定当时源码和二进制；开发、隔离验收、开发部署和生产切换分别判断。标为 `.local/` 或 `/tmp/` 的原始产物仅原机器保有，克隆仓库不携带；仓库中的脚本可用于复跑，不能因此声称旧原始输出可独立获取。后续记录方式见 [开发指南](development.md#文档归属)。
