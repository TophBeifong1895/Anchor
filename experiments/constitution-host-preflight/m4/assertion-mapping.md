# M4 断言与测试映射

基点 2cef07b。完整函数对应见 [机器映射](test-mapping.json)；删除测试 0，新增既有 ignore 0，业务断言放宽 0，未修改冻结快照。

| 编号 | 类别与范围 | 处置及等价覆盖 |
| --- | --- | --- |
| M4A1 | 状态词汇/HTTP，run_access/http.rs::detail_preserves_phase_json_bytes_for_actual_kernel_facts | 保留 error/recovery 键、字段顺序与逐字节比较；failed 样本 error 从恒空改为独立字面量 worker failed: recorded reason，与 Kernel 失败原因一致；其他相位仍空字符串。recovery 从 RunView 同结构投影提供，待恢复内容不减。 |
| M4A2 | 旧记录内部字段，tests.rs 的 created_records… | 开始时间观察仍检查 UTC；删除 updated 的构造断言，增加四个已删字段不序列化断言。更新时间已有 Host RunObservations 回归，不在 Graph 再造一份。 |
| M4A3 | 旧恢复容器，migrated_lifecycle::stopped_conversation_run_can_retain_unknown_recovery_attempts | 等价替换为 human_stop_retains_unknown_recovery_observation_in_kernel_history：实际 Code+Observation stop，再 Human stop，核对 key/attempt 原文留史和 Graph 事实不变。Host recovery 定向用例保留全部人工决定/不重放，新增在线与只读入口投影一致断言。 |
| M4A4 | 旧 touch/save 机制，migrated_lifecycle::persist_stamps_updated_and_keeps_memory_in_step_with_disk | 等价替换 kernel_history_commit_preserves_graph_admission_across_file_reload：实际 Kernel stop 递增资源版本，重新 FileStore 打开核对 resource/history 精确一致，准入时间和 Graph 内容不变。 |
| M4A5 | 旧 recovery/error 字段，migrated_reload、host_operations | 移除不存在容器的空值断言；仍以完整 Phase::Settled(Done) 或 Executing 确认无旧错误残留，invocation/input/同一产物/成本/不重放/中断 provenance 等业务断言保持。 |
| M4A6 | 测试只读观察，API conversations/run_abandon 与 run_access/recovery | .recovery 改为观察视图的 recovery()；决定从 runs.recovery_facts 查询而非记录搬运。保留放弃前后/重启/兄弟恢复事实一致及完整 key+attempt 幂等；失败诊断打印 Kernel Phase，成功断言不变。 |
| M4A7 | C6 format/旧字段，migrated_storage 和 duplicate decode | 当前 Graph schema 15；旧 1–14 和已移除 updated/error/recovery/recovery_submissions 均具名拒绝且 opaque Resource 不重写；重复字段检测使用当前格式，保留未知字段和非对象拒绝。 |
| M4A8 | 旧机制名称，migrated_* | Kernel FileStore/Scheduler/frontier/dispatch 替换 RunStore、GraphRunner、旧 cursor 等描述；subprocess --exact、临时目录及 guard 同步改名。C6 的 cursor payload、冻结场景 cursor 字段保留历史身份。 |
| M4A9 | 新增 Web 业务覆盖 | TimelinePreview 打开时间线条后断言 HTTP state.error 原文显示；RunDetailPage 明确显示 projection.reason；ChannelSessions 未知效果保留原因且只提供投影允许的放弃，不开放继续/退役/清理。原业务提示均保留。 |

冻结旧记录仍在 test_support/FrozenOldRecord，包含旧字段只用于历史证据；graph_record() 只取当前 Graph 事实。from_graph() 的历史观察为空，不伪造更新时间或恢复决定；冻结原始 30,000 例未修改、未重生成。
