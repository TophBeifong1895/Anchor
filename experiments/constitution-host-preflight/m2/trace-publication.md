# Goose trace 原子发布根因确认

读者和写者此前访问同一证据文件 inode：`store_evidence` 经 `write_durable` 执行 `fs::write`，先截断再写入。并发 `/runs/{id}` 可能在两步之间读到空白或部分 JSON，返回 `Goose trace resource is malformed`；写完后现场 JSON 又合法。这是已复现的机制根因。原首轮 500 的瞬时字节没有留存，不能声称逐字还原那次读取。

`490c000` 已把发布改为同目录临时文件 → 写入 → 文件 fsync → rename 原子替换 → 父目录 fsync。读者始终观察一个完整版本；本轮未再修改最终实现。

本轮临时恢复修复前的 `write_durable` 调用，执行同一 `trace_reader_retains_complete_snapshot_during_evidence_publication` 回归五次，均在并发读取的 JSON 断言稳定失败，exit 101；不是编译失败。恢复原子发布实现后，定向结果 1 / 0 / 0。

原始输出在 `/tmp/anchor-m2/section-10-12/trace-before-{1..5}.log`、`trace-before.json` 和 `trace-after.log`。失败日志包含大字节串，未入库；机器可读汇总见 [最终检查](completion-checks.json)。这次确认不把真实模型 Provider 或生产部署列为已验。
