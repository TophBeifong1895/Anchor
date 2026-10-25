//! The runs tools the assistant may call.
//!
//! Split out of the single dispatch in `pilot_tools`; the table there stays
//! the only description of what these tools accept.

use super::*;

impl PilotTools {
    pub(super) async fn run_control(
        &self,
        name: &str,
        arguments: &Value,
    ) -> Result<Value, ToolError> {
        self.require_active_turn()?;
        let run = parameter(arguments, "run")?;
        if !anchor_runtime::identifier::is_name(&run) {
            return Err(failed());
        }
        let operation = name.strip_prefix("run_").expect("matched Run control");
        self.application
            .control(&run, operation)
            .await
            .map_err(|failure| ToolError::Failed(format!("Run control rejected: {failure:?}")))?;
        Ok(json!({"run":run,"operation":operation,"requested":true}))
    }

    pub(super) async fn run_list(&self, _arguments: &Value) -> Result<Value, ToolError> {
        self.application
            .pilot_run_list()
            .await
            .map_err(|_| failed())
    }

    pub(super) async fn run_status(&self, arguments: &Value) -> Result<Value, ToolError> {
        let run = parameter(arguments, "run")?;
        if !anchor_runtime::identifier::is_name(&run) {
            return Err(failed());
        }
        self.application
            .pilot_run_status(&run)
            .await
            .map_err(|_| failed())
    }
}
