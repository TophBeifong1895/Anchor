# H1 标识符调用点分界

本表逐处记录 H1 改为引用共享原语的位置，也列入 H1 提交说明。行号对应本次交回源码。

- `is_plugin_id`：Plugin id、authoring 引用、Library 安装/回收、凭据/配置/探测、挂载、候选准备/审查/安装/卸载、渠道 Plugin 绑定。Graph Host 原 `valid_reference` 直接重导出同一原语，它原本还用于 MCP server 和逻辑配置字段名，保留同一严格规则。local input grant 名与旧 tool id 原本也要求首字符为字母或数字，同样调用严格原语，不改变其接受集合。
- `is_name`：Graph、Run、Turn、已发布 Artifact 和安全路径 component；凭据 `valid_target` 的 server/Graph 也保持名称规则。Run/渠道输入与删除意图的既有长度上限、node id 的分段和保留名判断仍留在原领域校验中。
- 删除的 `pilot_tools::identifier` 所有消费者均逐项选择上述规则；不保留从存储服务反向借用入口 helper 的路径。

| 规则 | 位置 | 使用点 |
| --- | --- | --- |
| `is_plugin_id` | [anchor-graph-host/src/resources.rs:5](../../../rust/anchor-graph-host/src/resources.rs#L5) | `导入`：`pub(crate) use anchor_runtime::identifier::is_plugin_id as valid_reference;` |
| `is_plugin_id` | [anchor-library/src/lib.rs:443](../../../rust/anchor-library/src/lib.rs#L443) | `validate_id`：`if anchor_runtime::identifier::is_plugin_id(id) {` |
| `is_plugin_id` | [anchor-library/src/reclaim.rs:291](../../../rust/anchor-library/src/reclaim.rs#L291) | `reclaim_locked`：`if !anchor_runtime::identifier::is_plugin_id(&id) {` |
| `is_plugin_id` | [anchor-runner-host/src/api.rs:800](../../../rust/anchor-runner-host/src/api.rs#L800) | `set_session_plugin`：`if !anchor_runtime::identifier::is_plugin_id(&plugin) {` |
| `is_plugin_id` | [anchor-runner-host/src/api/plugin_management/lifecycle.rs:346](../../../rust/anchor-runner-host/src/api/plugin_management/lifecycle.rs#L346) | `prepare_install`：`if !anchor_runtime::identifier::is_plugin_id(&body.id)` |
| `is_name` | [anchor-runner-host/src/api/runs.rs:169](../../../rust/anchor-runner-host/src/api/runs.rs#L169) | `delete_run`：`if !anchor_runtime::identifier::is_name(&id) {` |
| `is_name` | [anchor-runner-host/src/application/abandon.rs:79](../../../rust/anchor-runner-host/src/application/abandon.rs#L79) | `abandon_intent`：`if !anchor_runtime::identifier::is_name(run_id) {` |
| `is_name` | [anchor-runner-host/src/application/abandon.rs:106](../../../rust/anchor-runner-host/src/application/abandon.rs#L106) | `abandon_run`：`if !anchor_runtime::identifier::is_name(run_id) {` |
| `is_plugin_id` | [anchor-runner-host/src/application/channel_management.rs:38](../../../rust/anchor-runner-host/src/application/channel_management.rs#L38) | `validate`：`if !anchor_runtime::identifier::is_plugin_id(&self.plugin) {` |
| `is_name` | [anchor-runner-host/src/application/deletion.rs:359](../../../rust/anchor-runner-host/src/application/deletion.rs#L359) | `run_bytes`：`.filter(|id| anchor_runtime::identifier::is_name(id))` |
| `is_name` | [anchor-runner-host/src/application/graphs.rs:70](../../../rust/anchor-runner-host/src/application/graphs.rs#L70) | `graph_path_checked`：`if !anchor_runtime::identifier::is_name(name) {` |
| `is_name` | [anchor-runner-host/src/application/graphs.rs:143](../../../rust/anchor-runner-host/src/application/graphs.rs#L143) | `list_graphs`：`if anchor_runtime::identifier::is_name(&name)` |
| `is_name` | [anchor-runner-host/src/application/metadata.rs:215](../../../rust/anchor-runner-host/src/application/metadata.rs#L215) | `path`：`if !anchor_runtime::identifier::is_name(id) {` |
| `is_name` | [anchor-runner-host/src/application/run_access/kernel_runs.rs:368](../../../rust/anchor-runner-host/src/application/run_access/kernel_runs.rs#L368) | `kernel_owner`：`if !anchor_runtime::identifier::is_name(id) {` |
| `is_name` | [anchor-runner-host/src/application/run_access/reads.rs:7](../../../rust/anchor-runner-host/src/application/run_access/reads.rs#L7) | `reject_legacy`：`if !anchor_runtime::identifier::is_name(run_id) {` |
| `is_name` | [anchor-runner-host/src/application/run_access/run_observations.rs:51](../../../rust/anchor-runner-host/src/application/run_access/run_observations.rs#L51) | `accepted`：`.filter(|id| anchor_runtime::identifier::is_name(id))` |
| `is_name` | [anchor-runner-host/src/artifacts.rs:916](../../../rust/anchor-runner-host/src/artifacts.rs#L916) | `validate_component`：`if !anchor_runtime::identifier::is_name(value) {` |
| `is_name` | [anchor-runner-host/src/channel_inputs.rs:34](../../../rust/anchor-runner-host/src/channel_inputs.rs#L34) | `safe_run`：`run.len() <= 200 && anchor_runtime::identifier::is_name(run)` |
| `is_name` | [anchor-runner-host/src/channel_supervisor/management.rs:226](../../../rust/anchor-runner-host/src/channel_supervisor/management.rs#L226) | `assert_owner`：`if !anchor_runtime::identifier::is_name(graph) {` |
| `is_name` | [anchor-runner-host/src/channel_supervisor/management.rs:781](../../../rust/anchor-runner-host/src/channel_supervisor/management.rs#L781) | `saved_binding`：`if !anchor_runtime::identifier::is_name(graph) {` |
| `is_name` | [anchor-runner-host/src/channel_supervisor/management.rs:816](../../../rust/anchor-runner-host/src/channel_supervisor/management.rs#L816) | `read_facts`：`.filter(|graph| anchor_runtime::identifier::is_name(graph))` |
| `is_name` | [anchor-runner-host/src/credential_store.rs:117](../../../rust/anchor-runner-host/src/credential_store.rs#L117) | `valid_target`：`&& anchor_runtime::identifier::is_name(server)` |
| `is_name` | [anchor-runner-host/src/credential_store.rs:118](../../../rust/anchor-runner-host/src/credential_store.rs#L118) | `valid_target`：`&& anchor_runtime::identifier::is_name(graph)` |
| `is_name` | [anchor-runner-host/src/credential_store.rs:121](../../../rust/anchor-runner-host/src/credential_store.rs#L121) | `valid_target`：`!server.starts_with("channel_") && anchor_runtime::identifier::is_name(server)` |
| `is_plugin_id` | [anchor-runner-host/src/credential_store.rs:355](../../../rust/anchor-runner-host/src/credential_store.rs#L355) | `request`：`if !anchor_runtime::identifier::is_plugin_id(plugin)` |
| `is_plugin_id` | [anchor-runner-host/src/credential_store.rs:439](../../../rust/anchor-runner-host/src/credential_store.rs#L439) | `check_fact`：`|| !anchor_runtime::identifier::is_plugin_id(&fact.request.plugin)` |
| `is_plugin_id` | [anchor-runner-host/src/credential_store.rs:821](../../../rust/anchor-runner-host/src/credential_store.rs#L821) | `delete_plugin`：`if !anchor_runtime::identifier::is_plugin_id(plugin) {` |
| `is_plugin_id` | [anchor-runner-host/src/credential_store/management.rs:5](../../../rust/anchor-runner-host/src/credential_store/management.rs#L5) | `definition`：`if !anchor_runtime::identifier::is_plugin_id(plugin) {` |
| `is_plugin_id` | [anchor-runner-host/src/goose_acp/pilot_interaction.rs:337](../../../rust/anchor-runner-host/src/goose_acp/pilot_interaction.rs#L337) | `uninstall_with_answer`：`if !anchor_runtime::identifier::is_plugin_id(plugin) {` |
| `is_plugin_id` | [anchor-runner-host/src/goose_acp/pilot_interaction.rs:498](../../../rust/anchor-runner-host/src/goose_acp/pilot_interaction.rs#L498) | `install_arguments`：`Some(id) if anchor_runtime::identifier::is_plugin_id(id) => Some(id.to_owned()),` |
| `is_plugin_id` | [anchor-runner-host/src/goose_acp/pilot_interaction.rs:671](../../../rust/anchor-runner-host/src/goose_acp/pilot_interaction.rs#L671) | `install`：`.filter(|name| anchor_runtime::identifier::is_plugin_id(name))` |
| `is_plugin_id` | [anchor-runner-host/src/goose_acp/pilot_interaction/initialization.rs:18](../../../rust/anchor-runner-host/src/goose_acp/pilot_interaction/initialization.rs#L18) | `policy`：`.filter(|plugin| anchor_runtime::identifier::is_plugin_id(plugin))` |
| `is_name` | [anchor-runner-host/src/local_inputs.rs:163](../../../rust/anchor-runner-host/src/local_inputs.rs#L163) | `current`：`.filter(|name| anchor_runtime::identifier::is_name(name))` |
| `is_plugin_id` | [anchor-runner-host/src/local_inputs.rs:264](../../../rust/anchor-runner-host/src/local_inputs.rs#L264) | `read_grants`：`if !anchor_runtime::identifier::is_plugin_id(name) {` |
| `is_name` | [anchor-runner-host/src/pilot_attachments.rs:55](../../../rust/anchor-runner-host/src/pilot_attachments.rs#L55) | `directory`：`if turn.session != session.id || !anchor_runtime::identifier::is_name(&turn.id) {` |
| `is_name` | [anchor-runner-host/src/pilot_attachments.rs:399](../../../rust/anchor-runner-host/src/pilot_attachments.rs#L399) | `read`：`if !anchor_runtime::identifier::is_name(id) {` |
| `is_plugin_id` | [anchor-runner-host/src/pilot_tools/plugins.rs:25](../../../rust/anchor-runner-host/src/pilot_tools/plugins.rs#L25) | `plugin_candidate`：`.filter(|id| anchor_runtime::identifier::is_plugin_id(id))` |
| `is_plugin_id` | [anchor-runner-host/src/pilot_tools/plugins.rs:147](../../../rust/anchor-runner-host/src/pilot_tools/plugins.rs#L147) | `plugin_review`：`Some(id) if anchor_runtime::identifier::is_plugin_id(id) => id.to_owned(),` |
| `is_plugin_id` | [anchor-runner-host/src/pilot_tools/plugins.rs:152](../../../rust/anchor-runner-host/src/pilot_tools/plugins.rs#L152) | `plugin_review`：`.filter(|name| anchor_runtime::identifier::is_plugin_id(name))` |
| `is_name` | [anchor-runner-host/src/pilot_tools/runs.rs:16](../../../rust/anchor-runner-host/src/pilot_tools/runs.rs#L16) | `run_control`：`if !anchor_runtime::identifier::is_name(&run) {` |
| `is_name` | [anchor-runner-host/src/pilot_tools/runs.rs:36](../../../rust/anchor-runner-host/src/pilot_tools/runs.rs#L36) | `run_status`：`if !anchor_runtime::identifier::is_name(&run) {` |
| `is_name` | [anchor-runner-host/src/pilot_tools/sessions.rs:13](../../../rust/anchor-runner-host/src/pilot_tools/sessions.rs#L13) | `artifact_read`：`if !anchor_runtime::identifier::is_name(&run) {` |
| `is_plugin_id` | [anchor-runner-host/src/plugin_configuration.rs:63](../../../rust/anchor-runner-host/src/plugin_configuration.rs#L63) | `definition`：`if !anchor_runtime::identifier::is_plugin_id(plugin) {` |
| `is_plugin_id` | [anchor-runner-host/src/plugin_configuration_test.rs:20](../../../rust/anchor-runner-host/src/plugin_configuration_test.rs#L20) | `test`：`if !anchor_runtime::identifier::is_plugin_id(&plugin) {` |
| `is_name` | [anchor-runner-host/src/run_data.rs:124](../../../rust/anchor-runner-host/src/run_data.rs#L124) | `remove_legacy_conversation`：`if !stem.starts_with("np1-") || !anchor_runtime::identifier::is_name(stem) {` |
| `is_name` | [anchor-runner-host/src/run_data.rs:290](../../../rust/anchor-runner-host/src/run_data.rs#L290) | `artifact_paths`：`if anchor_runtime::identifier::is_name(id) {` |
| `is_name` | [anchor-runner-host/src/run_data.rs:414](../../../rust/anchor-runner-host/src/run_data.rs#L414) | `delete_run_data`：`if anchor_runtime::identifier::is_name(&record.run_id) {` |
| `is_name` | [anchor-runner-host/src/run_data.rs:467](../../../rust/anchor-runner-host/src/run_data.rs#L467) | `remove_run_files`：`if !anchor_runtime::identifier::is_name(run_id) {` |
| `is_name` | [anchor-runner-host/src/run_deletions.rs:98](../../../rust/anchor-runner-host/src/run_deletions.rs#L98) | `valid_id`：`value.len() <= 200 && anchor_runtime::identifier::is_name(value)` |
| `is_plugin_id` | [anchor-runner-host/src/tool_environment.rs:851](../../../rust/anchor-runner-host/src/tool_environment.rs#L851) | `load`：`if !anchor_runtime::identifier::is_plugin_id(id) {` |
| `is_plugin_id` | [anchor-runtime/src/graph/authoring.rs:227](../../../rust/anchor-runtime/src/graph/authoring.rs#L227) | `valid_plugin_ref`：`if crate::identifier::is_plugin_id(name) {` |
| `is_name` | [anchor-runtime/src/graph/logic.rs:643](../../../rust/anchor-runtime/src/graph/logic.rs#L643) | `validate_component`：`if !crate::identifier::is_name(s) {` |
| `is_name` | [anchor-runtime/src/graph/logic.rs:652](../../../rust/anchor-runtime/src/graph/logic.rs#L652) | `validate_node_id`：`.any(|part| !crate::identifier::is_name(part) || part == ".graph-calls")` |
