//! The sessions tools the assistant may call.
//!
//! Split out of the single dispatch in `pilot_tools`; the table there stays
//! the only description of what these tools accept.

use super::*;

impl PilotTools {
    pub(super) async fn artifact_read(&self, arguments: &Value) -> Result<Value, ToolError> {
        let run = parameter(arguments, "run")?;
        let node = parameter(arguments, "node")?;
        let path = parameter(arguments, "path")?;
        if !anchor_runtime::identifier::is_name(&run) {
            return Err(failed());
        }
        let record = self
            .application
            .run_views()
            .map_err(|_| failed())?
            .into_iter()
            .find(|(id, _)| id == &run)
            .ok_or_else(failed)?
            .1;
        let progress = record.progress();
        let result = progress
            .results
            .get(&node)
            .and_then(|results| results.last())
            .ok_or_else(failed)?;
        let artifacts = HostArtifacts::new(
            self.data_root.join("artifacts"),
            self.workspace_root.clone(),
        );
        let files = artifacts.files_path(&result.commit).map_err(|_| failed())?;
        let text = text_resource(&files, &path)?;
        Ok(json!({"run":run,"node":node,"path":path,"text":text}))
    }

    pub(super) async fn session_wait(&self, _arguments: &Value) -> Result<Value, ToolError> {
        let session = self
            .sessions
            .get(&self.owner, &self.session)
            .map_err(|_| failed())?;
        let retained = self
            .application
            .pilot_session_runs(&session.run_ids)
            .map_err(|_| failed())?;
        Ok(json!({"session":session,"runs":retained}))
    }
}
