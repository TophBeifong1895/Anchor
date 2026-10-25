# H3 后 main.rs 之外的环境读取

范围是 `anchor-runner-host/src` 的生产代码。按 `env::var`、`var_os`、`env_path` 调用及其参数来源核查；下表记录变量名，不读取当前环境值。四个 H3 变量均只在 `main.rs::HostConfiguration::from_env` 各读取一次，测试夹具也不再修改它们。

## 仍保留的生产读取

| 文件（相对 src/） | 变量或动态来源 |
| --- | --- |
| `api.rs` | `ANCHOR_RUNNER_BUNDLE_ROOT`、`ANCHOR_RUNNER_WORKSPACE_ROOT`、`ANCHOR_RUNNER_LISTEN`、`ANCHOR_API_KEYS`、`ANCHOR_RUNNER_SCHEDULES_PATH`、`ANCHOR_RUNNER_WEB_ROOT`（两个调用） |
| `api/assistant_recovery.rs` | `ANCHOR_ASSISTANT_AUTO_RESUME` |
| `api/oauth.rs`、`api/oauth/pilot.rs` | `ANCHOR_OAUTH_REDIRECT_URI` |
| `application.rs` | `ANCHOR_DEPENDENCIES_PYTHON`，经 `PYTHON_INTERPRETER_ENV` 常量 |
| `application/run_access/graph_policy.rs` | `ANCHOR_RUNNER_WORKSPACE_ROOT`，经 `env_path` |
| `application/session_calls.rs` | `ANCHOR_SESSION_HOST_URL`、`ANCHOR_SESSION_HOST_TOKEN` |
| `deployment.rs` | `ANCHOR_RUNNER_GRAPH_NAME`、`ANCHOR_RUNNER_BUNDLE_ROOT` |
| `execution.rs` | `ANCHOR_RUNNER_GRAPH_NAME` |
| `goose_acp.rs` | `ANCHOR_RUNNER_AGENT_RUNTIME`、`ANCHOR_GOOSE_BINARY`、`ANCHOR_GOOSE_ALLOW_SHARED_NETWORK` |
| `goose_acp/configuration.rs` | `ANCHOR_GOOSE_BINARY`、`ANCHOR_GOOSE_ALLOW_SHARED_NETWORK`、`ANCHOR_GOOSE_BINARY_SHA256`、`ANCHOR_GOOSE_ENABLED_EXTENSIONS`、`ANCHOR_GOOSE_RELAY_BINARY`、`ANCHOR_GOOSE_LOCAL_NETWORK`；fixture 模式的 `ANCHOR_GOOSE_OPENAI_HOST`、`ANCHOR_GOOSE_MODEL` |
| `model_configuration.rs` | `ANCHOR_MODEL_CONTEXT_WINDOW`、`ANCHOR_MODEL_MAX_TOKENS`、`ANCHOR_MODEL_DIRECT`，经 `ENV_FIELDS` |
| `runtime_configuration.rs` | `ANCHOR_DEPENDENCIES_PYTHON`、`ANCHOR_PILOT_PYTHON`、`ANCHOR_PILOT_PYTHON_ENABLED`、`ANCHOR_PILOT_RUSTUP`、`ANCHOR_PILOT_RUSTUP_HOME`、`ANCHOR_PILOT_CARGO_HOME`；外部字段检测还读 `ANCHOR_DEPENDENCIES_SOURCES`、`ANCHOR_DEPENDENCIES_UV`、`ANCHOR_DEPENDENCIES_UV_SHA256`、`ANCHOR_DEPENDENCIES_BUILD_ROOT` |
| `tool_environment.rs` | `ANCHOR_PILOT_RUSTUP`、`ANCHOR_PILOT_RUSTUP_HOME`、`ANCHOR_PILOT_CARGO_HOME`、`ANCHOR_PILOT_PYTHON`、`ANCHOR_PILOT_PYTHON_ENABLED`、`ANCHOR_DEPENDENCIES_PYTHON` |
| `dependency_provision/config.rs` | `ANCHOR_DEPENDENCIES_SOURCES`、`ANCHOR_DEPENDENCIES_UV`、`ANCHOR_DEPENDENCIES_UV_SHA256`、`ANCHOR_DEPENDENCIES_PYTHON`、`ANCHOR_DEPENDENCIES_BUILD_ROOT` |
| `local_inputs.rs` | `ANCHOR_RUNNER_LOCAL_INPUTS_ROOT` |
| `node_tools.rs` | `ANCHOR_NODE_COMMAND_TIMEOUT_SECONDS` |
| `tool_disclosure.rs` | `ANCHOR_NODE_ALWAYS_VISIBLE`、`ANCHOR_NODE_TOOL_DISCLOSURE`（两个调用） |
| `pilot_tools.rs` | `ANCHOR_RUNNER_FAKE_PLUGINS`、`ANCHOR_RUNNER_ALLOWED_COMMANDS` |
| `plugin_configuration_test.rs` | `ANCHOR_RUNNER_ALLOWED_COMMANDS`；这是生产连接测试服务，不是测试夹具 |
| `op.rs` | `ANCHOR_ROUTES` |
| `retention.rs` | `ANCHOR_RETENTION_DAYS`、`ANCHOR_RETENTION_MAX_GB`，经 `limit` |
| `plugin_configuration.rs` | `ANCHOR_PLUGIN_EXTERNAL_CONFIGURATION`；经 Plugin 声明与授权配置解析出的环境字段名，可能包含其他 `ANCHOR_*` |
| `credential_store.rs` | 经 `legacy_external_environment` 得到的、已授权的 Plugin 环境字段名，可能包含其他 `ANCHOR_*` |
| `channel_supervisor.rs` | `ANCHOR_WECOM_WS_URL`、`ANCHOR_WECOM_CONNECT_MS`、`ANCHOR_WECOM_ACK_MS`、`ANCHOR_WECOM_HEARTBEAT_MS`、`ANCHOR_WECOM_RECONNECT_MS`、`ANCHOR_WECOM_RECONNECT_MAX_MS`、`ANCHOR_WECOM_ACK_DELAY_MS`、`ANCHOR_WECOM_RECOVERY_WINDOW_MS`、`ANCHOR_WECOM_DEBUG`、`ANCHOR_WECOM_PROGRESS`、`ANCHOR_CHANNEL_WEBHOOK_TIMEOUT_MS` |

`ModelRegistry::from_env(false)` 的原始环境适配器仍可读取 `ANCHOR_MODEL_DIRECT`、`ANCHOR_MODEL_URL`、`ANCHOR_MODEL_API_KEY`、`ANCHOR_MODEL_NAME`、`ANCHOR_MODEL_WIRE_API`、`ANCHOR_MODEL_ALIASES`、`ANCHOR_MODEL_CONTEXT_WINDOW`、`ANCHOR_MODEL_MAX_TOKENS`。当前生产调用 `from_env` 只用于 fixture 模式；平台模型执行从保存字段直接调用 `from_fields`，没有恢复为环境模型配置。原始适配器保留供现有 Goose 配置契约测试使用。

上述其他变量的读取边界本轮未迁移。`PATH`、`HOME` 等非 `ANCHOR_*` 读取不在此表范围。Plugin 动态环境字段不可能仅靠静态名称穷举，已按读取入口单列。

## 只在测试中的额外读取

普通测试的环境保存/恢复不计入生产读取。容易被纯文本搜索误认为生产读取的例子包括：`durable_write.rs` 的 `ANCHOR_DURABLE_WRITE_CRASH_DIR`、`dependency_provision/fetch.rs` 的 `ANCHOR_DEPENDENCIES_LIVE_MIRROR`、`dependency_provision/test_support.rs` 的 `ANCHOR_REQUIRE_ENV_BUILDER` / `ANCHOR_TEST_BWRAP`，以及 `node_host/resolver_tests/runtime.rs` 的测试环境快照。测试中保留的全局锁保护其他仍由环境读取的部署字段。
