# 固定 Goose 原生能力激活实验

2026-10-13。用户同意先验证 Goose 原生动态扩展，再比较 Anchor 聚合 MCP 接入。实验只新增一个 Rust probe target，没有修改 Runtime、权限、数据格式或部署；这不是懒连接产品交付。

## 可复现入口与身份

[goose_native_activation_spike.rs](../../rust/anchor-runner-host/tests/goose_native_activation_spike.rs) 使用现有确定性 Provider fixture、真实固定 Goose ACP 与本地 HTTP MCP。Probe 支持多个 ACP outstanding ID，仅用于测量 out-of-band mutation；现有生产 `AcpConnection` 保持一次一个请求，没有增加另一套生产 Agent loop。

Goose v1.53.0，binary SHA256 `71e76c412597b2ecd96ed20d0706e7666f31c018216e7cb5d65c5ca5c44824a7`。从已有构建缓存读取源 archive，先校验 SHA256 `85cc5e76a12e364032df4bec0ed0738a910c3e12ec12731b4405d1fdb18d96ae`，相关原文保存在本地 `sources/` 缓存；提交保留[固定版源码位置与摘要](sources-manifest.json)，可从第一方源码复核。运行和 test binary/source identity、实际调用及结果见 [results.json](results.json)。

从仓库根实际执行，均退出 0：

```bash
PATH=/home/mansteinl/.cargo/bin:$PATH ANCHOR_GOOSE_BINARY=/home/mansteinl/Anchor/.local/rust/bin/goose cargo test --manifest-path rust/Cargo.toml -p anchor-runner-host --test goose_native_activation_spike --locked --offline fixed_goose_ -- --ignored --nocapture
PATH=/home/mansteinl/.cargo/bin:$PATH cargo clippy --manifest-path rust/Cargo.toml -p anchor-runner-host --test goose_native_activation_spike --locked --offline -- -D warnings
PATH=/home/mansteinl/.cargo/bin:$PATH cargo fmt --manifest-path rust/Cargo.toml --all -- --check
```

**2 passed / 0 failed / 5 filtered out**；filtered 是该 target 复用 fixture 自带的其他测试，不宣称运行。没有重跑未修改的 Host all-targets 或 Web。日志 `/tmp/anchor-goose-native-activation-{spike,clippy,fmt}.log`。

## 实际结果

| 问题 | 实测结果 | 对 Anchor 的意义 |
| --- | --- | --- |
| ACP 在 prompt 内添加扩展 | 当工具尚在等待时 add 成功，选中服务开始连接；本轮下一次模型请求没有新 schema，下一 prompt 才有 | ACP add 成功不能等同于 Agent 已学到新增工具；生产 transport 也未支持同时发第二请求 |
| 原生 Extension Manager 激活 | 配置中 disabled 的 fixture 服务在 enable 前未连接；模型调用 manager 后，同 prompt 下一模型请求出现新 schema，实际 echo 成功 | 原生管理工具具有可复用的轮内刷新行为，完整 Native 接入仍需授权适配 |
| 单服务大量工具 | 激活后全部 1,004 个工具定义进入下一模型请求；该请求 tools JSON 合计 223,104 B | 按扩展激活没有解决单个大 MCP 的 schema 成本；字节不是 tokenizer 测量 |
| 管理边界 | 原生 manager 实际可以停用 `tom`，不改变 fixture 全局配置文件 | 直接开放管理器会让模型改变 Anchor 校准过的扩展集合，不能将其状态当宿主权限 |
| 原生 form elicitation | MCP 请求经 Goose ACP，probe 返回 accept/approve=true，实际工具反馈继续同 prompt | 原生确认基础传输可用；未验证 Pilot UI、删除前置条件或持久问题事实 |
| 业务错误 | MCP `isError=true` 的标记进入后续模型工具反馈 | 普通 native 调用保留该错误；未对 Code Mode 的结果转换签认 |
| 调用身份 | 实际 MCP context 包含 agent-session-id、agent-tool-call-request-id、agent-working-dir | 具有对接宿主观察的原生来源；未接入 Anchor receipt/trace |
| 取消 | 原 prompt 返回 cancelled，HTTP MCP handler 观察到 cancellation | 只证明本次活动 HTTP 调用取消，不是首次连接取消或 stdio 子进程回收 |
| 会话重载 | 成功添加的扩展被保留，新 Goose 进程 load 后重连，并实际再调用 | 原生 Session 记载的是已启用扩展；恢复前仍须按当前宿主授权校准 |
| 移除 | 显式 remove 后下一 prompt 的 schema 不再包含该服务 | 不是原 prompt 中即时撤销的完整验证 |

原始 evidence 根 `/tmp/anchor-goose-acp-spike/evidence/integration/593339/`，两个场景为 `goose-native-activation-spike`、`goose-native-manager-spike`。各根有 `evidence.json`、`provider.json`、`native/acp-*.json` 和 `native/stderr-*.log`；最后 stderr 均空，未见 Goose panic。进程退出后只读检查 `/proc`，无存活实验进程。当前部署 PID 184037 executable SHA256 保持 `984578f85d9bc460a8868f53236f8e99c04bc61d69f616993adcb67cb42d2f15`。

初稿以 `tools/list` 计数作为 add 连接完成信号，断言失败；实际 add 已握手，inventory 在需要准备工具时才获取。最终改为 initialize 计数，重连也使用 initialize 观察；较早概要的 `reload_reconnects_selected=false` 是错误指标，不作为最终结论。实验代码的 RMCP 构造/API 类型错误已在首次成功运行前修正，未修改框架或生产协议。

## 原因与路线判断

固定版 `Agent::reply_internal` 在入口准备本轮工具，只有识别到原生 `extensionmanager__manage_extensions` 调用成功等特定路径时，才设置 `tools_updated` 并重建工具/提示；out-of-band ACP add 的 cache invalidation 不会刷新这份已经取得的本轮列表。`Agent::add_extension` 将启用结果持久化到原生 Session，load 使用保留集合。原生 manager 的 enable 从 Goose configuration 查名字，disable 接受其他已启用扩展；其管理域并非 Anchor Library / invocation 授权集合。

本实验支持的选择：继续通过 Goose 原生 ACP 管理 Agent 会话、历史、压缩、取消和扩展校准；Plugin 披露及按需使用保持 Anchor 授权投影，连接沿现有成熟 `rmcp` adapter 推进。若未来原生接口提供授权激活和可靠的轮内 inventory 更新，再在适配层替换对应机制。具体后续边界见 [能力复用与按需激活路线](../../docs/goose-native-capability-reuse-plan.md)。

## 未覆盖

没有真实 Provider、Anchor Host/GraphRunner 集成、独立 Graph 发行、实际业务、stdio 沙箱/子进程回收、媒体、OAuth/凭据、启动期取消、并发多次首次连接或热更新竞态。没有升级 Goose、启用 Code Mode 或修改当前部署。A222 的真实 Provider 和现有聚合 MCP 验收可用于比较，但不能回填为本实验的新原生路径验收。
