//! Node workspace and frozen input authorization.
use super::*;

impl HostIoResolver {
    pub(crate) fn local_inputs_configured(&self) -> bool {
        self.local_inputs.configured()
    }

    /// Prepare the one writable tree this invocation owns.
    ///
    /// A handed-over assistant Run seeds its work node's scene from the Run it
    /// took the instance over from; every other preparation is the plain
    /// unbound/committed-seed behaviour. The seed decision is a durable host fact
    /// (`crate::assistant::handover_workspace_source`), never graph content or
    /// user input.
    pub(crate) fn prepare_node_workspace(
        &self,
        key: &InvocationKey,
        inputs: &[CommitRef],
    ) -> Result<PathBuf, String> {
        let seed = crate::assistant::handover_workspace_source(
            self.local_inputs.state_root(),
            key,
            &self.artifacts,
        )?;
        if let Some(source) = seed {
            let prior = self
                .run_store
                .load(&source.run_id)
                .map_err(|error| error.to_string())?
                .ok_or("assistant workspace source Run is missing")?;
            let current = self
                .run_store
                .load(&key.run_id)
                .map_err(|error| error.to_string())?
                .ok_or("assistant workspace target Run is missing")?;
            self.artifacts
                .prepare_workspace_from_handover(
                    key,
                    inputs,
                    &source,
                    &prior.snapshot,
                    &current.snapshot,
                )
                .map_err(|error| error.to_string())
        } else {
            self.artifacts
                .prepare_workspace_from(key, inputs, None)
                .map_err(|error| error.to_string())
        }
    }

    pub(crate) fn persistent_workspace(&self, key: &InvocationKey) -> Result<bool, String> {
        crate::assistant::source(self.local_inputs.state_root(), key).map(|source| source.is_some())
    }

    pub(crate) fn bind_local_inputs(
        &self,
        record: &GraphRunRecord,
        graph: Option<&str>,
    ) -> Result<(), String> {
        let existing = self
            .run_store
            .load(&record.run_id)
            .map_err(|error| error.to_string())?;
        if let Some(existing) = &existing
            && (existing.snapshot != record.snapshot || existing.input != record.input)
        {
            return Err("Run local input identity differs from the admitted snapshot/input".into());
        }
        self.local_inputs.freeze(
            &record.run_id,
            &record.graph_digest,
            &record.snapshot,
            graph,
            existing.is_none(),
        )
    }

    pub(crate) fn verify_local_inputs(&self, record: &GraphRunRecord) -> Result<(), String> {
        self.local_inputs.verify(record)?;
        if self.local_inputs.configured() {
            let metadata =
                crate::application::metadata::load(self.local_inputs.state_root(), &record.run_id)
                    .map_err(|error| format!("local input Run identity unavailable: {error:?}"))?
                    .ok_or("local input authorization requires immutable Run Graph metadata")?;
            if metadata.graph_digest != record.graph_digest {
                return Err(
                    "local input Run Graph metadata differs from the admitted snapshot".into(),
                );
            }
            self.local_inputs.freeze(
                &record.run_id,
                &record.graph_digest,
                &record.snapshot,
                Some(&metadata.graph),
                false,
            )?;
        }
        Ok(())
    }

    pub(super) fn local_input_mounts(
        &self,
        key: &InvocationKey,
    ) -> Result<Vec<anchor_runtime::ReadOnlyInput>, String> {
        let record = self
            .run_store
            .load(&key.run_id)
            .map_err(|error| error.to_string())?
            .ok_or("local input mounts require a durable admitted Run")?;
        // Old Runs could not receive local inputs. Only their empty authority
        // may be recorded during migration; new grants require a new Run.
        if !self.local_inputs.configured() {
            self.local_inputs.freeze(
                &record.run_id,
                &record.graph_digest,
                &record.snapshot,
                None,
                false,
            )?;
        }
        self.verify_local_inputs(&record)?;
        let mut mounts = self.local_inputs.mounts(&record, key)?;
        if let Some(mount) = self.channel_inputs(key)?.mount {
            mounts.push(mount);
        }
        mounts.extend(
            self.pending_channel_inputs(key)?
                .into_iter()
                .filter_map(|inputs| inputs.mount),
        );
        Ok(mounts)
    }
}
