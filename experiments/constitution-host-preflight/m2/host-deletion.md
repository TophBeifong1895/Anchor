# M2 Host 删除接入

Kernel Run/Graph 删除现通过 application/run_access 的删除预留和 FileStore::remove 完成；create、control、drive 注册与预留使用同一锁序，排队 worker 在删除后重新读取权威资源，不执行节点。旧路径仍保留默认。单 Run 保留原有无 worker 删除边界；Graph 级联使用 phase/worker 并保留 Session 投递、对话继任、wait-child、调用引用及产物身份保护。

10 项定向测试通过，含工作中及已结算仍排空的 worker 拒绝、删除预留期间控制和派发拒绝、产物损坏导致清理失败后可重试、重启、对话 paused/budget 保护、级联与其他 Graph 隔离。wait-child 补充断言单独通过。旧业务断言未改；application/deletion.rs 围栏从 3 降到 1，围栏测试文件未修改。两处对话后继/子 Run 查询改走已有 load_run 门面，类型围栏计数未变。

执行命令、结果与原始日志 SHA256 见 checks.json 的 host_deletion_slice。旧 deletion 过滤 46、graph_delete 过滤 13 及 fence 1 项也通过；这些早期命令只有会话控制台结果，未单独保留原始日志，不另给哈希。Host bin/tests clippy 通过。完整 M2、workspace/Web/Goose 验收尚未执行。

保留原清理边界：准备执行和删除交错可能留下可回收的准备目录，但删除后 worker 不执行节点；未进入 Graph 结果的冻结产物沿用既有孤立产物清理边界。删除存储能力和名称长度限制见 deletion-boundary.md。
