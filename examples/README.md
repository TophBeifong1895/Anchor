# 随源码提供的 Graph 与 Plugin

执行 `./scripts/dev.sh start` 后，首次启动会把 [starter.json](starter.json) 中的示例导入可编辑 catalog，并把官方 Plugin 安装到 Library。网页中的 Graph 和插件库可直接查看、修改；安装插件本身不会执行工作流、连接外部账号或启用企微机器人。

## 从哪个 Graph 开始

| Graph | 用途 | 运行前需要 |
| --- | --- | --- |
| `call-report`、`call-worker` | 主工作流生成报告，调用子工作流处理，再读取返回文件。建议先运行 `call-report` | 无需模型；输入可使用 `{"report_code":"DEMO-001"}` |
| `revise-loop` | 写作、评审、反馈返工和最终文件 | 在模型设置保存 default Flash |
| `parallel-audit` | 两个独立审查分支并行，完成后综合报告 | default Flash |
| `plugin-research` | 单节点学术调研，生成有来源的研究笔记 | default Flash；任务联网 |
| `deep-academic-research` | 多节点研究、质疑、评审和报告，可能进行多轮模型调用 | default Flash；任务联网 |
| `wecom-persistent-assistant` | 常驻企微助手模板：这张 Graph 配一个机器人，每个会话一个常驻实例 | default Flash；在 Agent 节点所挂 `wecom` Plugin 详情的「此 Graph 的机器人」中保存成员、设置凭据并启用 |

## 已安装的 Plugin

| Plugin | 随附能力 | 配置 |
| --- | --- | --- |
| `network` | 隔离网络中的 HTTP 请求、文件下载与联网说明 | 无需单独账号；受会话和节点网络权限限制 |
| `academic-research` | 学术检索、原文读取和研究 Skill | 无需仓库附带的账号；访问情况取决于实际论文服务 |
| `docmost` | 页面 MCP 与图片附件工具 | 在插件详情填写 Docmost URL 和 API key |
| `wecom` | 企微工具、Gateway 和使用说明 | 机器人按 Graph 配置，见上；默认不连接 |
| `anchor-steward` | Anchor 插件开发和适配说明 | 无额外服务配置 |

插件的本地 Rust 工具随源码首次准备编译和安装，用户不用手动复制二进制。模型、插件连接和渠道配置仍由网页管理，不附带任何凭据。

## 用户修改与其他示例

首次导入保留已有的同名 Graph/Plugin。导入完成后，源码更新与重启不会覆盖这些副本，也不会恢复用户已删除的条目。可按需在网页修改或另建 Graph，Plugin 的更新和卸载沿用插件库的管理流程。

`setup` 会准备插件及工具；Graph 需要 Host API，在 `start/restart` 中导入。显式配置的 entry bundle、自定义 Host 或外部服务不自动导入。完整预构建平台包的内容沿用其发行清单，本说明描述源码启动。

[graphs/](graphs/) 仍提供其他现有示例。RSI、周报及 Docmost 周报包装图依赖明确授权的本机输入目录和附加工具，按 [RSI](../docs/rsi.md)、[周报](../docs/weekly-work-report.md) 配置后导入；Python 依赖探针用于开发验证，按需从插件库安装并准备 Python 环境。这些内容未列为默认上手项目。
