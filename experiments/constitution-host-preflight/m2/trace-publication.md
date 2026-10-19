# Goose trace 原子发布根因确认

修复前读者和写者访问同一证据文件 inode：`store_evidence` 经 `write_durable` 执行 `fs::write`，先截断再写入。并发 `/runs/{id}` 可能在两步之间读到空白或部分 JSON，返回 `Goose trace resource is malformed`；写完后现场 JSON 又合法。这是已复现的机制根因。原首轮 500 的瞬时字节没有留存，不能声称逐字还原那次读取。

`490c000` 已把发布改为同目录临时文件 → 写入 → 文件 fsync → rename 原子替换 → 父目录 fsync。读者始终观察一个完整版本。收尾合入 master d042ebe 后，store_evidence 改为调用共享 write_durable；保持同样原子替换顺序，并覆盖全部六处 durable writer 调用，保留既有权限、创建 umask 和发布失败清理。

本轮临时恢复修复前的 `write_durable` 调用，执行同一 `trace_reader_retains_complete_snapshot_during_evidence_publication` 回归五次，均在并发读取的 JSON 断言稳定失败，exit 101；不是编译失败。恢复原子发布实现后，定向结果 1 / 0 / 0。

原始输出在 `/tmp/anchor-m2/section-10-12/trace-before-{1..5}.log`、`trace-before.json` 和 `trace-after.log`。失败日志包含大字节串，未入库；机器可读汇总见 [最终检查](completion-checks.json)。这次确认不把真实模型 Provider 或生产部署列为已验。

合并后的三项 trace 回归全部通过（单快照、32 次交错发布、64 次持续并发）。单快照测试显式设置已有文件为 0600 后验证替换保留权限，既有断言不变；不把首次创建的 umask 契约与权限保留混为一谈。见 completion-checks.json 中 trace-postmerge.log，最终 workspace 同时覆盖共享 writer 的权限与失败清理回归。
