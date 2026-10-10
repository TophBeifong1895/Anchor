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
        if !identifier(&run) {
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
        let runs = self
            .application
            .records()
            .map_err(|_| failed())?
            .into_iter()
            .map(|(id, record)| json!({"run":id,"status":record.status}))
            .collect::<Vec<_>>();
        Ok(json!({"runs":runs}))
    }

    pub(super) async fn run_status(&self, arguments: &Value) -> Result<Value, ToolError> {
        let run = parameter(arguments, "run")?;
        if !identifier(&run) {
            return Err(failed());
        }
        let record = self
            .application
            .records()
            .map_err(|_| failed())?
            .into_iter()
            .find(|(id, _)| id == &run)
            .ok_or_else(failed)?
            .1;
        Ok(json!({"run":run,"record":record}))
    }
}
