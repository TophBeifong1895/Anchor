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
        let result = record
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
            .records()
            .map_err(|_| failed())?
            .into_iter()
            .filter(|(id, _)| session.run_ids.contains(id))
            .map(|(id, record)| json!({"run":id,"record":record}))
            .collect::<Vec<_>>();
        Ok(json!({"session":session,"runs":retained}))
    }
}
