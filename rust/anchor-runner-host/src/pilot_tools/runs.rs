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
        let effective = if operation == "pause" {
            "stop"
        } else {
            operation
        };
        self.application
            .control(&run, effective)
            .await
            .map_err(|failure| ToolError::Failed(format!("Run control rejected: {failure:?}")))?;
        Ok(json!({"run":run,"operation":effective,"alias":operation == "pause","requested":true}))
    }

    pub(super) async fn run_list(&self, _arguments: &Value) -> Result<Value, ToolError> {
        let mut runs = Vec::new();
        for (id, record) in self.application.records().map_err(|_| failed())? {
            let metadata = self.application.metadata(&id).map_err(|_| failed())?;
            let projection = self
                .application
                .project_run(&record, metadata.as_ref())
                .await
                .map_err(|_| failed())?;
            runs.push(json!({"run":id,"projection":projection}));
        }
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
        let metadata = self.application.metadata(&run).map_err(|_| failed())?;
        let projection = self
            .application
            .project_run(&record, metadata.as_ref())
            .await
            .map_err(|_| failed())?;
        let mut progress = serde_json::to_value(record).map_err(|_| failed())?;
        progress
            .as_object_mut()
            .expect("record object")
            .remove("status");
        Ok(json!({"run":run,"record":progress,"projection":projection}))
    }
}
