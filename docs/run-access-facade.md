# Run 访问边界

## 当前实现

本分支按迁移任务书 §10.8 合入 F0（`e3a5778`），未合入 F1（`06f7b8a` 及后续提交）。原生 Run 所有权、读取视图、旧状态适配、兼容桥和宿主时间观察已移入 `application/run_access/`；业务围栏上限由 master 的 224 降为本分支实际 192，运行时允许范围与上游完全一致。M1 的三相位和独立 pause 保持，未改变持久格式、权限、锁顺序或恢复行为。

当前模块只归拢本分支已有实现。master 的 F1 完成后再以其接口、谓词名称和构造方式一次性对齐，不能把本次移动解释为 F1/F2 已完成。规则位点与未收敛边界见 [M1.5 测量](../experiments/constitution-host-preflight/m15/measurement.md)。

[围栏测试](../rust/anchor-runner-host/tests/run_access_fence.rs) 扫描 `anchor-runner-host/src/**/*.rs`，对 `RunStatus`、`GraphRunRecord`、`RunStore`、`FileRunStore`、`GraphRunner`、`RunCursor` 六种内部类型的精确标识符逐次计数。[业务基线](../rust/anchor-runner-host/run-access-fence.txt) 是逐文件上限：超过上限或清单外业务文件出现引用会失败；下降或删除文件时提示同步降低清单，不自动改写它。

扫描排除 `tests/`、`*tests.rs` 和精确 `#[cfg(test)]` 所保护的项，包括外部测试模块；跳过该项后恢复扫描，不丢弃同文件后续生产代码。注释和字面量不计数，导入、完整路径、别名声明和宏输入中的类型标识符计数。不展开宏或执行名称解析，不追踪别名使用、glob 导入和推断类型；因此围栏不是所有直接字段读写的静态证明，F1/F2 仍须迁移这些路径并封装内部可变状态。

## 运行时范围

永久允许范围固定在测试中，不由业务基线扩展：

| 范围 | 归属理由 |
| --- | --- |
| `application/run_access.rs`、`application/run_access/` | 本分支原生 Run 所有权与兼容实现的边界；尚未对齐完整 F1/F2 接口 |
| `execution.rs` | 共享 Runner 的执行装配与调用 |
| `main.rs` | 宿主、CLI 与 Runtime 的组合根 |
| `node_host.rs`、`node_host/` | NodeExecutionPort 的实现与装配；根文件和同名目录共同形成一个运行时模块，不另豁免业务路径 |
| `run_data.rs` | Runtime 产物及历史数据访问 |

`anchor-graph-host` 整个 crate 属于运行时侧，不在本围栏范围内。其余 Host 文件默认为业务侧，包括当前没有类型标识符、但仍通过推断类型读写 Run 的文件。

## HTTP 字节基线

[新增 HTTP 回归](../rust/anchor-runner-host/src/api/tests/run_access_json.rs) 固定 Run 身份与时间，逐字节比较 `/runs` 的原始响应，不先解析或重新序列化 JSON。覆盖十种旧存储状态及其非活动投影；按已批准 S19–S24、S26 改为 phase/waiter/outcome/reason、四动作及宿主更新时间，仍逐字节比较字段顺序，不保留旧 status、terminal、quiescent 或 needs_attention 字段。等待恢复的样本保留有效 cursor 与待处理 attempt。

当前序列化保留字段插入顺序。后续 F1 接口对齐仍须比较原始响应字节；本次词汇断言调整已登记在工作记录 M1.5。新增基线不替代后续详情、Pilot、活动执行、并发控制和恢复路径的原有断言与专项核查。

## 尚未关闭

- F0 围栏与新相位 HTTP 字节比较通过；最终 workspace 与 Goose 验收见 [M1.5 验证](../experiments/constitution-host-preflight/m15/verification.md)。Goose 使用 G0 完整基线对照，不将 master 已有失败记为本分支新增。
- F1/F2 尚未合入。本分支已有读取投影仍有过渡边界，写入控制仍含旧规则；逐规则数量见测量，不以类型引用数替代规则收敛证明。
- 后续 M2 的原生访问代码继续放在门面或既有运行时范围，不提高业务计数，也不新增豁免。M3 仍需等待完整 F2 合入后才能切换并删除旧 Runner。
