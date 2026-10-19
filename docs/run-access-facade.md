# Run 访问边界

## 当前实现

当前 master 仅建立 F0 围栏及 HTTP 字节基线；Run 的生产实现、持久格式、状态词汇、权限、锁顺序和恢复行为均未改变。`application/run_access` 门面尚未实现，不能把围栏通过视为读取、规则或写入已收敛。

[围栏测试](../rust/anchor-runner-host/tests/run_access_fence.rs) 扫描 `anchor-runner-host/src/**/*.rs`，对 `RunStatus`、`GraphRunRecord`、`RunStore`、`FileRunStore`、`GraphRunner`、`RunCursor` 六种内部类型的精确标识符逐次计数。[业务基线](../rust/anchor-runner-host/run-access-fence.txt) 是逐文件上限：超过上限或清单外业务文件出现引用会失败；下降或删除文件时提示同步降低清单，不自动改写它。

扫描排除 `tests/`、`*tests.rs` 和精确 `#[cfg(test)]` 所保护的项，包括外部测试模块；跳过该项后恢复扫描，不丢弃同文件后续生产代码。注释和字面量不计数，导入、完整路径、别名声明和宏输入中的类型标识符计数。不展开宏或执行名称解析，不追踪别名使用、glob 导入和推断类型；因此围栏不是所有直接字段读写的静态证明，F1/F2 仍须迁移这些路径并封装内部可变状态。

## 运行时范围

永久允许范围固定在测试中，不由业务基线扩展：

| 范围 | 归属理由 |
| --- | --- |
| `application/run_access.rs`、`application/run_access/` | 读取和写入门面的预留模块边界；不表示当前已实现 |
| `execution.rs` | 共享 Runner 的执行装配与调用 |
| `main.rs` | 宿主、CLI 与 Runtime 的组合根 |
| `node_host.rs`、`node_host/` | NodeExecutionPort 的实现与装配；根文件和同名目录共同形成一个运行时模块，不另豁免业务路径 |
| `run_data.rs` | Runtime 产物及历史数据访问 |

`anchor-graph-host` 整个 crate 属于运行时侧，不在本围栏范围内。其余 Host 文件默认为业务侧，包括当前没有类型标识符、但仍通过推断类型读写 Run 的文件。

## HTTP 字节基线

[新增 HTTP 回归](../rust/anchor-runner-host/src/api/tests/run_access_json.rs) 固定 Run 身份与时间，逐字节比较 `/runs` 的原始响应，不先解析或重新序列化 JSON。覆盖十种既有状态及其非活动投影，包含控制动作、终态、静止、注意提示和字段顺序。等待恢复的样本保留有效 cursor 与待处理 attempt。

当前序列化保留字段插入顺序。F1 不得仅验证 JSON 对象等价，或在将状态序列化移入门面时调整字段次序。新增基线不替代后续详情、Pilot、活动执行、并发控制和恢复路径的原有断言与专项核查。

## 尚未关闭

- F0 定向围栏和 HTTP 字节比较通过，不代表整体验收通过；固定 Goose 的 ACP 扩展初始化失败，独立复跑仍复现。实际命令、首轮失败、独立复跑和已中断套件的范围见[本机验证](../.local/run-access-facade-20261019/README.md)，原始日志仅在本机保有。
- F1 仍需实现只读 `RunView`、唯一相位映射、逐规则谓词及统一对外序列化；逐规则的迁移前/后独立判断位置数尚未交付，不以围栏标识符数替代该度量。
- F2 仍需按意图封装 Run 写入；不得用类型重命名、放宽运行时豁免或直接暴露原始 record 来降低围栏数。迁移前继续沿用现有事实所有者、租约、执行登记及持久化顺序。
