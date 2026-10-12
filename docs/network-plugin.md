# Pilot 任务联网能力

联网 Plugin 的固定 id 是 `network`。它是任务联网的启用入口，并提供通用 HTTP 请求和下载工具。实际网络权限由 Host/Sandbox 执行；学术调研等业务 Plugin 继续使用自己的客户端，不需要把请求转交给联网工具。

## 启用与授权

Session 分别保存主动挂载开关和 `network_allowed`。后者是用户是否允许任务联网的上限，默认允许满足已挂载 Plugin 的联网需求；没有启用者时不会因此开放网络。关闭联网 Plugin 的主动开关只撤销主动启用，明确禁止联网使用卡片上的「禁止联网」。

| 主动挂载联网 | 有已挂载的联网依赖者 | 允许联网 | 结果 |
| --- | --- | --- | --- |
| 否 | 否 | 是 | 未启用，任务工具在无网沙箱中运行 |
| 是 | 否 | 是 | 主动启用，通用联网工具可用 |
| 否 | 是 | 是 | 依赖启用，业务客户端与通用联网工具可用 |
| 是 | 是 | 是 | 主动启用且被依赖 |
| 任意 | 任意 | 否 | 禁止联网；需要联网的 Plugin 工具不装配，离线工具继续运行 |

联网能力要求 Library 已安装有效的 `network` Plugin。缺少它时，联网依赖者显示不可用，不自行安装。依赖启用由当前挂载集合推导，不额外写入 `plugin_mounts`；关闭最后一个依赖者且没有主动启用后，联网能力自动停止。卸载联网 Plugin 时，依赖者所在的 Session、未结束 Run 和引用 Graph 都纳入已有卸载检查。

联网开关与 Plugin 挂载从下一轮生效，同一轮重试使用首次保存的快照。`state/plugin-mounts/<session>/<turn>.json` 格式 2 同时保存挂载和授权，格式 1 的旧文件兼容读取。无法读取快照时，挂载沿用已有兼容回退，网络权限拒绝开放。

模型 Provider 的连接属于宿主控制通道，不受任务联网开关影响。带 Session 的 Graph 节点还必须满足原有节点 `network:true` 上限；无 Session 的独立 Graph 保留原有节点网络授权契约。这次改动没有将独立 Graph 的权限迁移成 Pilot Session 开关。

## Plugin 声明

清单顶层 `"network": true` 表示该 Plugin 的任务工具需要联网，不是授权。省略时为 false，非布尔值拒绝。远程 HTTP/SSE MCP 的连接本身需要联网，Host 自动识别这一需求。该字段不改变 `dependencies.python` 的包依赖语义，也不引入通用 Plugin 依赖解析器。

官方学术调研和企业微信声明了联网需求；Docmost 的远程 MCP 自动产生需求。实际 stdio 进程按本轮有效授权使用 Bubblewrap 的网络隔离；HTTP MCP 与 OAuth 装配使用同一授权。授权覆盖本次 Agent/任务工具的网络访问，当前没有按域名或单个插件划分出站网络。

## 通用工具和候选包

`network-http_request` 支持 HTTP(S) 方法、headers 和文本 body，返回最终 URL、状态码、响应 headers、文本及截断标记。默认读取 64 KiB，最多 1 MiB，超时 30 秒，最多跟随 5 次重定向。

`network-http_download` 下载到工作区相对路径，默认最多 16 MiB，上限 64 MiB。下载超限不会留下目标文件，已有文件不覆盖。目录通过文件描述符逐级打开且不跟随符号链接，写入限于 Host 授权的工作区。

构建与生成可审查候选：

```sh
cargo build --manifest-path rust/Cargo.toml -p anchor-network-tools --release --locked
rust/target/release/anchor-network-tools package-plugin /absolute/path/to/candidate/network
```

目标父目录必须已存在，目标目录必须不存在。命令生成清单、Skill 和 `bin/anchor-network-tools`；安装仍使用既有 Library 审查/安装流程，编译产物沿用已有审查规则。部署后将学术调研升级到包含 `network:true` 的清单，再从会话插件面板挂载。

验收证据和未覆盖范围记录在 [Pilot 开发台账](pilot-development-plan.md)，不把浏览器夹具或本地服务当作真实模型/公网验收。
