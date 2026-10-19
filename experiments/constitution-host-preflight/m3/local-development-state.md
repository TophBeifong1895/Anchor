# M3 本地开发状态

所有新 Run 使用 GraphRunRecord format 14。旧 `runs/<id>.json` 只检查存在性，返回 `legacy_run_record`，不解析旧数据；M2 `kernel-runs` 内 format 13 的 Graph 状态同样具名不接受。按任务书 §10.13 不提供迁移工具，也不把旧执行记录当 Goose 恢复游标。本轮没有清理任何已有开发状态。

旧 Run 可能被 Session、助手 current、投递和原生会话引用，因此只删除 `runs/` 或 `kernel-runs/` 会留下悬空引用。最稳妥的本地开发重置办法是停止当前开发服务，然后使用一对全新的 state/workspace 根。默认 catalog 与 Library 继续沿用原目录；旧状态保留原样，可另行备份或检查。

例如在仓库根目录手工执行，选择尚不存在的目录名，并在后续 start/restart 使用相同配置：

```sh
bash scripts/dev.sh stop
export ANCHOR_RUNNER_STATE_ROOT="$PWD/.local/rust/state-m3"
export ANCHOR_RUNNER_WORKSPACE_ROOT="$PWD/.local/rust/workspaces-m3"
bash scripts/dev.sh start
```

这会使用新的 Session、助手绑定、投递及模型/渠道托管配置；需要在网页重新设置。原 catalog/Library 不随该操作删除，原凭据文件也不被复制或覆盖。若现有服务自定义了 `ANCHOR_DEV_ROOT`、catalog 或 Library 根，应继续使用原配置；上述操作是开发者可选步骤，不是部署或自动升级方案。

若仅为丢弃整个实验环境而手工清理，范围是该实验自己的 **完整 state 根及 workspace 根**，先停止其 Host 并备份。state 内包括 `runs/`、`kernel-runs/`、Run metadata/观察/控制意图、Artifact、节点事实、原生 trace/会话以及 Session 关联；不要把共享 `io-harness/` 当成单 Run 目录，也不要把 catalog、Library 或另一实例的数据根一起删除。保留部分 Session 或原生历史需要逐引用核查，不由本里程碑执行。
