//! An invocation keeps its authorized connection identity across recovery.
//! Secrets and secret hashes are deliberately absent; rotating a key within the
//! same owner, target and credential reference remains possible.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionBindings {
    format: u32,
    key: InvocationKey,
    owner: String,
    bindings: BTreeMap<String, String>,
}

fn retain(
    state: &Path,
    key: &InvocationKey,
    owner: &str,
    bindings: &BTreeMap<String, String>,
) -> Result<(), String> {
    let path = state
        .join("facts")
        .join(format!("{}.plugin-configuration.json", fact_stem(key)));
    let _lease = crate::credential_store::lease(state)?;
    if let Some(previous) = crate::credential_store::load::<ConnectionBindings>(&path)? {
        if previous.format != 1 || previous.key != *key || previous.owner != owner {
            return Err("retained Plugin connection ownership is invalid".into());
        }
        if previous.bindings != *bindings {
            return Err("Plugin connection target or configuration source changed; restore its previous configuration to continue this execution, or start a new Run".into());
        }
        return Ok(());
    }
    if bindings.is_empty() {
        return Ok(());
    }
    crate::credential_store::save(
        &path,
        &ConnectionBindings {
            format: 1,
            key: key.clone(),
            owner: owner.into(),
            bindings: bindings.clone(),
        },
    )
}

impl HostIoResolver {
    pub(super) async fn retain_plugin_configuration(
        &self,
        key: &InvocationKey,
        bindings: BTreeMap<String, String>,
    ) -> Result<(), String> {
        let state = self.local_inputs.state_root().to_path_buf();
        let owner = self
            .oauth_owner_for_run(&key.run_id)?
            .unwrap_or_else(|| "local".into());
        let key = key.clone();
        tokio::task::spawn_blocking(move || retain(&state, &key, &owner, &bindings))
            .await
            .map_err(|_| "Plugin connection binding unavailable".to_owned())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_connection_rejects_scope_or_target_changes_but_allows_new_invocations() {
        let root = tempfile::tempdir().unwrap();
        let key = InvocationKey {
            run_id: "run".into(),
            graph_digest: "graph".into(),
            node_id: "worker".into(),
            invocation: 1,
        };
        let original = BTreeMap::from([("demo".into(), "original-connection".into())]);
        retain(root.path(), &key, "owner-a", &original).unwrap();
        // A recreated resolver/process observes the same durable identity.
        retain(root.path(), &key, "owner-a", &original).unwrap();
        assert!(retain(root.path(), &key, "owner-b", &original).is_err());
        let changed = BTreeMap::from([("demo".into(), "another-target".into())]);
        assert!(retain(root.path(), &key, "owner-a", &changed).is_err());
        assert!(retain(root.path(), &key, "owner-a", &BTreeMap::new()).is_err());
        let next = InvocationKey {
            invocation: 2,
            ..key
        };
        retain(root.path(), &next, "owner-a", &changed).unwrap();
    }
}
