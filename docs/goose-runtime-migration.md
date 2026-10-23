# Goose ACP 接入决定

决定日期：2026-10-06。状态：有效。Anchor 使用固定 Goose 作为 Agent 后端，AgentNode 与 Pilot 共用 ACP/MCP 接入。当前实现见 [架构](architecture.md#goose-与恢复)，验收范围见 [验收摘要](pilot-development-plan.md)。

## 原因

Goose 已提供 Agent loop、Provider、原生会话、加载、取消和 compaction；Anchor 已拥有 GraphRunner 与产品事实。复用这些能力，可以把接入工作限定在节点端口、授权工具和产品观察，不再维护一套通用 Agent 框架或永久多后端选择器。

## 边界与后果

- Goose 拥有模型历史与上下文，Anchor 拥有 Graph/Run/Artifact、Session/Turn、Library 和宿主授权。Goose session/recipe 不替代产品对象。
- Responses 推理回传修复维护为固定 Goose 源码补丁，由原生 Thinking 的可选 provider metadata 保存完整 reasoning 项，并由原生 Provider 回传；Host 不读写推理历史或建立旁路缓存。构建记录同时固定上游源码、补丁和产物摘要。原版 1.53.0 到该修订仅允许新 invocation 加载已核对身份的旧原生历史，同 invocation 仍保持原二进制绑定。
- ACP 是 Agent 执行交互边界，MCP 是授权工具边界；Kernel 的稳定节点契约不依赖 Goose 私有类型。AgentNode/Pilot 共用适配与同一个 Runner。
- 新建或加载原生历史后，Host 在提示模型前校准扩展与授权。提示词、cwd、ACP permission 和旧会话扩展都不能代替 Sandbox 权限。
- 恢复同一 invocation 保留原生 Session 与 binary/model/endpoint 绑定，不静默更换。用户更新模型设置后，新 Turn/invocation 可在加载原生历史后通过 Goose 模型配置接口切换；当前执行不受影响，不创建替代历史。未知工具效果先保存观察，再由 Agent 核查现场；不自动重放、不承诺 exactly-once。
- 节点通过独立完成工具提交结构化结果和最近授权工具批次的实际回执，不依赖并行工具到达顺序。固定 Goose 薄补丁将宿主校验成功的完成工具接到原生历史保存后的结束边界，不追加模型请求；普通文本、猜测回执、取消或断连不能替代完成。工具关闭等待实际 handler 和所属进程收束，不能提前释放执行所有权。
- 提问、媒体、反馈、并行、恢复与 UI 都保持产品契约，分别验收；接口存在和真实 Provider 通过是不同结论。
- 官方 Host/工具/开发检查使用 Rust，WebUI 使用 React/TypeScript。第三方 Plugin 的解释器是授权运行资源，语言由作者决定。
- 旧执行器源码和依赖退出维护树，旧工作记录保留原身份，不能转换成 Goose 恢复游标或与新 backend 双写。

## 预算取舍

累计请求/token/金额预算暂不作为交付门槛，不为精确 usage 建模型代理或计量平台。已有字段未启用时明确告知，不把 usage 观察称为已强制预算，也不以任意轮数代替任务完成。

取消、协议/内存上界、进程收束和显式操作超时继续生效。固定版能力不足时先说明具体缺口，优先公开接口与薄适配，不通过私有 API 或重建 Agent loop 补齐。

## 后续查证

[原迁移计划](archive/development-2026-10-13/goose-runtime-migration.md) 保留 G0–G3、阶段验收和当时部署边界，不再用于安排开工。Runtime 回归入口见 [回归指南](runtime-contract-tests.md)；MCP 按需连接仍是 [独立未实施路线](goose-native-capability-reuse-plan.md)。新部署边界见 [部署指南](rust-production-deployment.md)，旧 Python 数据不提供迁移工具。
