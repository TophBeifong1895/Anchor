//! Current owner-bound configuration, independent of historical Pilot requests.
use super::*;

fn definition(library: &Path, plugin: &str) -> Result<PluginDefinition, String> {
    if !crate::pilot_tools::identifier(plugin) {
        return Err("invalid credential Plugin ID".into());
    }
    FilePluginCatalog::new(library)
        .definition(plugin)
        .map_err(|_| "credential target Plugin is unavailable".into())
}

fn check_secret(
    secret: &Secret,
    owner: &str,
    plugin: &str,
    target: &str,
    field: &str,
) -> Result<(), String> {
    if secret.format != 1
        || secret.owner != owner
        || secret.plugin != plugin
        || secret.server != split_target(target).0
        || secret.field != field
        || secret.value.is_empty()
    {
        return Err("stored credential identity is invalid".into());
    }
    Ok(())
}

/// Only a new successful set/rotation acknowledges an existing request. Merely
/// finding a saved value must never satisfy a request to replace that value.
pub(crate) fn resolve_pending_locked(
    data: &Path,
    sessions: &SessionStore,
    owner: &str,
    definition: &PluginDefinition,
    target: &str,
    field: &str,
    digest: &str,
) -> Result<(), String> {
    let configuration_version = if target.starts_with("channel_") {
        None
    } else {
        Some(crate::plugin_configuration::request_version_locked(
            data,
            &binding_owner(owner),
            definition,
        )?)
    };
    for session in sessions
        .list(owner)
        .map_err(|_| "credential request sessions unavailable")?
    {
        let root = requests_root(data, owner, &session);
        if private_parent(&root.join("probe"))?.is_none() {
            continue;
        }
        for entry in fs::read_dir(&root).map_err(|_| "credential requests unavailable")? {
            let path = entry.map_err(|_| "credential request unavailable")?.path();
            let Some(mut fact) = load::<RequestFact>(&path)? else {
                continue;
            };
            check_fact(&fact, owner, &session)?;
            if fact.request.status != "pending"
                || fact.request.plugin != definition.id
                || fact.request.server != target
                || fact.request.field != field
                || fact.digest != definition.digest
                || fact.authorization_digest != digest
                || fact.request.expires_at < chrono::Utc::now().timestamp()
                || path != request_path(data, owner, &session, &fact.request.id)?
            {
                continue;
            }
            if let Some(current) = &configuration_version
                && fact.configuration_version.as_ref().is_none_or(|previous| {
                    previous.revision.checked_add(1) != Some(current.revision)
                        || previous.binding_digest != current.binding_digest
                })
            {
                continue;
            }
            if sessions
                .get_turn(owner, &session.id, &fact.request.turn)
                .map_err(|_| "credential request Turn unavailable")?
                .status
                == TurnStatus::Running
            {
                fact.request.status = "configured".into();
                save(&path, &fact)?;
            }
        }
    }
    Ok(())
}

/// Metadata contains no stored values and makes no claim about provider validity.
pub(crate) fn credentials(
    data: &Path,
    library: &Path,
    owner: &str,
    plugin: &str,
) -> Result<Value, String> {
    let definition = definition(library, plugin)?;
    let _lease = lease(data)?;
    project(data, owner, &definition, &plugin_targets(&definition))
}

/// The Plugin's shared targets. A bot's channel credentials are listed by its
/// Graph instead, see [`channel_credentials`].
fn plugin_targets(definition: &PluginDefinition) -> BTreeSet<String> {
    definition
        .mcp_servers
        .iter()
        .map(|server| server.name.clone())
        .collect()
}

/// One bot Graph's channel credential state; values are never returned.
pub(crate) fn channel_credentials(
    data: &Path,
    library: &Path,
    owner: &str,
    plugin: &str,
    target: &str,
) -> Result<Value, String> {
    let definition = definition(library, plugin)?;
    let _lease = lease(data)?;
    project(
        data,
        owner,
        &definition,
        &BTreeSet::from([target.to_owned()]),
    )
}

fn targets_for(definition: &PluginDefinition, target: &str) -> BTreeSet<String> {
    if split_target(target).1.is_some() {
        BTreeSet::from([target.to_owned()])
    } else {
        plugin_targets(definition)
    }
}

fn project(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
    targets: &BTreeSet<String>,
) -> Result<Value, String> {
    let owner = binding_owner(owner);
    let mut records = Vec::new();
    for target in targets {
        let (fields, digest) = target_authorization(definition, target)?;
        if fields.is_empty() {
            continue;
        }
        let mut records_fields = Vec::new();
        for field in fields {
            if crate::plugin_configuration::logical_field(definition, target, &field).is_some() {
                continue;
            }
            let external = crate::plugin_configuration::is_external(definition, target, &field)?;
            let state = if external {
                "deployment"
            } else {
                match load::<Secret>(&secret_path(data, &owner, &definition.id, target, &field))? {
                    None => "missing",
                    Some(secret) => {
                        check_secret(&secret, &owner, &definition.id, target, &field)?;
                        if secret.authorization_digest == digest {
                            "configured"
                        } else {
                            "stale"
                        }
                    }
                }
            };
            records_fields.push(json!({"field":field,"state":state,"editable":!external}));
        }
        if records_fields.is_empty() {
            continue;
        }
        records.push(json!({"target":target,"kind":if target.starts_with("channel_") {"channel"} else {"mcp"},
            "authorization_digest":digest,"fields":records_fields}));
    }
    Ok(json!({"plugin":definition.id,"targets":records}))
}

/// Both the standalone management endpoint and Pilot submit use this mutation.
/// The API adapter also holds the catalog guard/reference lease while calling it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn configure(
    data: &Path,
    library: &Path,
    owner: &str,
    plugin: &str,
    target: &str,
    field: &str,
    authorization_digest: &str,
    value: Option<&str>,
    sessions: &SessionStore,
) -> Result<Value, String> {
    let definition = definition(library, plugin)?;
    let _lease = lease(data)?;
    let mut projection = project(data, owner, &definition, &targets_for(&definition, target))?;
    let warning = mutate_locked(
        data,
        owner,
        &definition,
        target,
        field,
        authorization_digest,
        value,
        sessions,
    )?;
    for record in projection["targets"]
        .as_array_mut()
        .ok_or("credential targets unavailable")?
    {
        if record["target"] != target {
            continue;
        }
        for record_field in record["fields"]
            .as_array_mut()
            .ok_or("credential fields unavailable")?
        {
            if record_field["field"] == field {
                record_field["state"] = json!(if value.is_some() {
                    "configured"
                } else {
                    "missing"
                });
            }
        }
    }
    projection["warning"] = json!(warning);
    Ok(projection)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn mutate_locked(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
    target: &str,
    field: &str,
    expected_authorization: &str,
    value: Option<&str>,
    sessions: &SessionStore,
) -> Result<Option<String>, String> {
    if !valid_target(target) || !valid_field(field) {
        return Err("invalid credential target or field".into());
    }
    let (fields, digest) = target_authorization(definition, target)?;
    if !fields.contains(field) {
        return Err("credential field is not declared by this Plugin target".into());
    }
    if digest != expected_authorization {
        return Err("Plugin authorization changed; reload credentials before saving".into());
    }
    if crate::plugin_configuration::is_external(definition, target, field)? {
        return Err("this credential field is managed externally by the deployment".into());
    }
    if let Some(logical) = crate::plugin_configuration::logical_field(definition, target, field) {
        crate::plugin_configuration::set_legacy_locked(
            data,
            &binding_owner(owner),
            definition,
            logical,
            value,
        )?;
        let mut warning = false;
        if value.is_some() {
            for (mapped_target, mappings) in &definition.configuration.environment {
                for (environment, mapped_field) in mappings {
                    if mapped_field == logical {
                        warning |= resolve_pending_locked(
                            data,
                            sessions,
                            owner,
                            definition,
                            mapped_target,
                            environment,
                            &digest,
                        )
                        .is_err();
                    }
                }
            }
        }
        return Ok(warning
            .then(|| "凭据已保存，但部分会话配置请求未能同步；请重新读取会话状态后核查。".into()));
    }
    let request_owner = owner;
    let owner = binding_owner(owner);
    let path = secret_path(data, &owner, &definition.id, target, field);
    if let Some(value) = value {
        if value.is_empty() || value.len() > 65536 || value.contains('\0') {
            return Err("credential value must be nonempty and bounded".into());
        }
        if !target.starts_with("channel_") {
            crate::plugin_configuration::invalidate_legacy_locked(data, &owner, definition)?;
        }
        save(
            &path,
            &Secret {
                format: 1,
                owner,
                plugin: definition.id.clone(),
                server: split_target(target).0.into(),
                field: field.into(),
                authorization_digest: digest.clone(),
                value: value.into(),
            },
        )?;
        let synchronization = resolve_pending_locked(
            data,
            sessions,
            request_owner,
            definition,
            target,
            field,
            &digest,
        );
        Ok(synchronization
            .err()
            .map(|_| "凭据已保存，但部分会话配置请求未能同步；请重新读取会话状态后核查。".into()))
    } else {
        if let Some(secret) = load::<Secret>(&path)? {
            check_secret(&secret, &owner, &definition.id, target, field)?;
            if !target.starts_with("channel_") {
                crate::plugin_configuration::invalidate_legacy_locked(data, &owner, definition)?;
            }
            fs::remove_file(&path).map_err(|_| "credential clearing failed")?;
            fs::File::open(path.parent().ok_or("credential parent unavailable")?)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| "credential clearing sync failed")?;
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn management_shares_pilot_secrets_and_preserves_request_history() {
        let (root, sessions, session, turn) = super::super::tests::fixture();
        let data = root.path().join("state");
        let field = "ANCHOR_TEST_PRIVATE_CREDENTIAL";
        let request = request(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &turn,
            "demo",
            "api",
            field,
            "configure",
        )
        .unwrap();
        submit(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &request.id,
            "first-private-value",
        )
        .unwrap();
        let metadata = credentials(&data, root.path(), "owner-a", "demo").unwrap();
        assert_eq!(metadata["targets"][0]["fields"][0]["state"], "configured");
        assert!(!metadata.to_string().contains("first-private-value"));
        let digest = metadata["targets"][0]["authorization_digest"]
            .as_str()
            .unwrap();
        configure(
            &data,
            root.path(),
            "owner-a",
            "demo",
            "api",
            field,
            digest,
            Some("rotated-private-value"),
            &sessions,
        )
        .unwrap();
        let definition = FilePluginCatalog::new(root.path())
            .definition("demo")
            .unwrap();
        assert_eq!(
            environment_values(&data, &binding_owner("owner-a"), &definition, "api").unwrap()
                [field],
            "rotated-private-value"
        );
        assert_eq!(
            credentials(&data, root.path(), "owner-b", "demo").unwrap()["targets"][0]["fields"][0]
                ["state"],
            "missing"
        );
        configure(
            &data,
            root.path(),
            "owner-a",
            "demo",
            "api",
            field,
            digest,
            None,
            &sessions,
        )
        .unwrap();
        assert_eq!(
            credentials(&data, root.path(), "owner-a", "demo").unwrap()["targets"][0]["fields"][0]
                ["state"],
            "missing"
        );
        assert_eq!(
            get(&data, &sessions, "owner-a", &session, &request.id)
                .unwrap()
                .status,
            "configured"
        );
    }

    #[test]
    fn standalone_rotation_completes_pending_requests_but_never_uses_a_preexisting_value() {
        let (root, sessions, session, turn) = super::super::tests::fixture();
        let data = root.path().join("state");
        let field = "ANCHOR_TEST_PRIVATE_CREDENTIAL";
        let metadata = credentials(&data, root.path(), "owner-a", "demo").unwrap();
        let digest = metadata["targets"][0]["authorization_digest"]
            .as_str()
            .unwrap();
        configure(
            &data,
            root.path(),
            "owner-a",
            "demo",
            "api",
            field,
            digest,
            Some("old-private-value"),
            &sessions,
        )
        .unwrap();
        let request = request(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &turn,
            "demo",
            "api",
            field,
            "replace existing",
        )
        .unwrap();
        assert_eq!(
            get(&data, &sessions, "owner-a", &session, &request.id)
                .unwrap()
                .status,
            "pending"
        );
        configure(
            &data,
            root.path(),
            "owner-b",
            "demo",
            "api",
            field,
            digest,
            Some("other-owner"),
            &sessions,
        )
        .unwrap();
        assert_eq!(
            get(&data, &sessions, "owner-a", &session, &request.id)
                .unwrap()
                .status,
            "pending"
        );
        configure(
            &data,
            root.path(),
            "owner-a",
            "demo",
            "api",
            field,
            digest,
            Some("new-private-value"),
            &sessions,
        )
        .unwrap();
        assert_eq!(
            get(&data, &sessions, "owner-a", &session, &request.id)
                .unwrap()
                .status,
            "configured"
        );
        // An already-open form cannot overwrite the value saved elsewhere.
        assert!(
            submit(
                &data,
                root.path(),
                &sessions,
                "owner-a",
                &session,
                &request.id,
                "late-private-value"
            )
            .is_err()
        );
        let definition = FilePluginCatalog::new(root.path())
            .definition("demo")
            .unwrap();
        assert_eq!(
            environment_values(&data, &binding_owner("owner-a"), &definition, "api").unwrap()
                [field],
            "new-private-value"
        );
    }

    #[test]
    fn saved_secret_is_reported_with_warning_if_request_synchronization_is_unavailable() {
        let (root, sessions, session, _) = super::super::tests::fixture();
        let data = root.path().join("state");
        save(
            &requests_root(&data, "owner-a", &session).join("malformed.json"),
            &json!({"invalid":"request"}),
        )
        .unwrap();
        let metadata = credentials(&data, root.path(), "owner-a", "demo").unwrap();
        let digest = metadata["targets"][0]["authorization_digest"]
            .as_str()
            .unwrap();
        let result = configure(
            &data,
            root.path(),
            "owner-a",
            "demo",
            "api",
            "ANCHOR_TEST_PRIVATE_CREDENTIAL",
            digest,
            Some("saved-private-value"),
            &sessions,
        )
        .unwrap();
        assert_eq!(result["targets"][0]["fields"][0]["state"], "configured");
        assert!(result["warning"].as_str().unwrap().contains("凭据已保存"));
        assert!(!result.to_string().contains("saved-private-value"));
        let definition = FilePluginCatalog::new(root.path())
            .definition("demo")
            .unwrap();
        assert_eq!(
            environment_values(&data, &binding_owner("owner-a"), &definition, "api").unwrap()["ANCHOR_TEST_PRIVATE_CREDENTIAL"],
            "saved-private-value"
        );
    }

    #[test]
    fn changed_authorization_is_stale_and_old_form_cannot_write() {
        let (root, sessions, _, _) = super::super::tests::fixture();
        let data = root.path().join("state");
        let metadata = credentials(&data, root.path(), "owner-a", "demo").unwrap();
        let digest = metadata["targets"][0]["authorization_digest"]
            .as_str()
            .unwrap();
        configure(
            &data,
            root.path(),
            "owner-a",
            "demo",
            "api",
            "ANCHOR_TEST_PRIVATE_CREDENTIAL",
            digest,
            Some("private-value"),
            &sessions,
        )
        .unwrap();
        fs::write(root.path().join("plugins/demo/plugin.json"), json!({"name":"Demo","mcpServers":{"api":{"url":"https://other.example/mcp","bearer_token_env_var":"ANCHOR_TEST_PRIVATE_CREDENTIAL"}}}).to_string()).unwrap();
        assert_eq!(
            credentials(&data, root.path(), "owner-a", "demo").unwrap()["targets"][0]["fields"][0]
                ["state"],
            "stale"
        );
        assert!(
            configure(
                &data,
                root.path(),
                "owner-a",
                "demo",
                "api",
                "ANCHOR_TEST_PRIVATE_CREDENTIAL",
                digest,
                Some("wrong"),
                &sessions
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn deployment_environment_is_reported_but_cannot_be_overwritten_or_cleared() {
        let _env = crate::PROCESS_ENV_LOCK.lock().await;
        const FIELD: &str = "ANCHOR_TEST_MANAGED_EXTERNALLY_ONLY";
        const EXTERNAL: &str = "ANCHOR_PLUGIN_EXTERNAL_CONFIGURATION";
        struct Restore(Option<std::ffi::OsString>, Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    match self.0.take() {
                        Some(value) => std::env::set_var(FIELD, value),
                        None => std::env::remove_var(FIELD),
                    }
                    match self.1.take() {
                        Some(value) => std::env::set_var(EXTERNAL, value),
                        None => std::env::remove_var(EXTERNAL),
                    }
                }
            }
        }
        let _restore = Restore(std::env::var_os(FIELD), std::env::var_os(EXTERNAL));
        let (root, sessions, _, _) = super::super::tests::fixture();
        fs::rename(
            root.path().join("plugins/demo"),
            root.path().join("plugins/deployment-demo"),
        )
        .unwrap();
        fs::write(root.path().join("plugins/deployment-demo/plugin.json"), json!({"name":"Demo","mcpServers":{"api":{"url":"https://example.com/mcp","bearer_token_env_var":FIELD}}}).to_string()).unwrap();
        unsafe {
            std::env::set_var(FIELD, "deployment-secret-never-public");
            std::env::set_var(
                EXTERNAL,
                json!({"deployment-demo": {format!("api.{FIELD}"): FIELD}}).to_string(),
            );
        }
        let data = root.path().join("state");
        let projection = credentials(&data, root.path(), "owner-a", "deployment-demo").unwrap();
        assert_eq!(projection["targets"][0]["fields"][0]["state"], "deployment");
        assert_eq!(projection["targets"][0]["fields"][0]["editable"], false);
        assert!(!projection.to_string().contains("deployment-secret"));
        let digest = projection["targets"][0]["authorization_digest"]
            .as_str()
            .unwrap();
        assert!(
            configure(
                &data,
                root.path(),
                "owner-a",
                "deployment-demo",
                "api",
                FIELD,
                digest,
                Some("replace"),
                &sessions
            )
            .is_err()
        );
        assert!(
            configure(
                &data,
                root.path(),
                "owner-a",
                "deployment-demo",
                "api",
                FIELD,
                digest,
                None,
                &sessions
            )
            .is_err()
        );
    }
}
