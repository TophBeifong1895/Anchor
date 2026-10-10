//! The environment tools the assistant may call.
//!
//! Split out of the single dispatch in `pilot_tools`: this family grew a
//! requirement, two readers and an impact report, and keeping it beside the
//! Graph and Run tools made one file hold four unrelated jobs.

use super::*;

impl PilotTools {
    pub(super) async fn environment_list(&self, _arguments: &Value) -> Result<Value, ToolError> {
        let store = anchor_library::EnvironmentStore::new(&self.library_root);
        let names = store
            .names()
            .map_err(|_| failed())?
            .into_iter()
            .map(|entry| json!({"name":entry.name,"id":entry.id,"version":entry.version}))
            .collect::<Vec<_>>();
        let environments = store
                    .list()
                    .map_err(|_| failed())?
                    .into_iter()
                    .map(|published| {
                        json!({"id":published.id,"bytes":published.bytes,"files":published.files})
                    })
                    .collect::<Vec<_>>();
        Ok(json!({"names":names,"environments":environments}))
    }

    pub(super) async fn environment_request(&self, arguments: &Value) -> Result<Value, ToolError> {
        // The model may ask for an environment and explain why; it may not
        // publish anything, so this is a Mutate tool rather than a Confirm
        // one: it writes only its own durable request. Approval fields are
        // refused here just as they are in the confirmed tools.
        self.require_active_turn()?;
        if let Some(key) = arguments.as_object().and_then(|object| {
            object.keys().find(|key| {
                !matches!(
                    key.as_str(),
                    "name" | "purpose" | "interpreter" | "packages" | "notes"
                )
            })
        }) {
            return Err(rejected(format!(
                "environment_request does not accept `{key}`"
            )));
        }
        let name = parameter(arguments, "name")?;
        if !anchor_runtime::graph::is_environment_name(&name) {
            return Err(rejected(
                "name must be at most 64 characters of letters, digits, dot, dash and underscore",
            ));
        }
        let purpose = parameter(arguments, "purpose")?;
        if purpose.trim().is_empty() || purpose.len() > 4096 {
            return Err(rejected("purpose must be non-empty and bounded"));
        }
        let interpreter = optional_bounded(arguments, "interpreter", 128)?;
        if let Some(constraint) = interpreter.as_deref()
            && !crate::environment_provision::is_interpreter_constraint(constraint)
        {
            // The deployment builds on one interpreter, so this names the version
            // it must be. A specifier like `>=3.11` could never be satisfied and
            // would only be discovered when someone asked to provision it.
            return Err(rejected(
                "interpreter must be a version prefix like `3.12`, not a specifier",
            ));
        }
        let notes = optional_bounded(arguments, "notes", 4096)?;
        let packages = match arguments.get("packages") {
            None => Vec::new(),
            Some(Value::Array(items)) if items.len() <= 200 => items
                .iter()
                .map(|item| {
                    item.as_str()
                        .filter(|package| {
                            !package.trim().is_empty()
                                && package.len() <= 256
                                && package.chars().all(|character| {
                                    character.is_ascii_alphanumeric()
                                        || "._-<>=!~*+,".contains(character)
                                })
                        })
                        .map(str::to_owned)
                        .ok_or_else(|| rejected("packages must be bounded name strings"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(rejected("packages must be an array of at most 200")),
        };
        // The assistant may not ask to change its own dependencies either,
        // and "its own" includes the Graphs it calls.
        let own = self
            .application
            .assistant_uses_environment_name(&self.graph_name, &name)
            .map_err(|error| match error {
                crate::application::ApplicationError::Invalid(reason) => rejected(reason),
                _ => failed(),
            })?;
        if own {
            return Err(rejected(format!(
                "`{name}` provides this assistant's own dependencies; ask the operator to change it through the operator interface"
            )));
        }
        let ask = crate::application::environments::EnvironmentRequirementAsk {
            requester: crate::application::environments::EnvironmentPlanRequester {
                owner: self.owner.clone(),
                session: self.session.clone(),
                turn: self.turn.clone(),
            },
            purpose,
            interpreter,
            packages,
            notes,
        };
        let requirement = self
            .application
            .ask_environment_requirement(&self.library_root, &name, ask)
            .map_err(|error| match error {
                // The model can act on both of these, so surface them.
                crate::application::ApplicationError::Invalid(reason)
                | crate::application::ApplicationError::Storage(reason) => {
                    ToolError::Failed(reason)
                }
                _ => failed(),
            })?;
        let references = self
            .application
            .environment_references(&name)
            .ok()
            .map(|references| {
                // The coverage travels with the counts: `{nodes:0,servers:0}` from
                // a partial scan must not read as "nobody uses it".
                json!({
                    "nodes": references.nodes.len(),
                    "servers": references.servers.len(),
                    "coverage": references.coverage
                })
            })
            .unwrap_or(Value::Null);
        Ok(json!({
            "requested": true,
            "name": name,
            "asks": requirement.requests.len(),
            "observed": requirement.observed,
            "referenced_by": references
        }))
    }

    pub(super) async fn environment_references(
        &self,
        arguments: &Value,
    ) -> Result<Value, ToolError> {
        let name = parameter(arguments, "name")?;
        let references = self
            .application
            .environment_references(&name)
            .map_err(|_| failed())?;
        serde_json::to_value(&references).map_err(|_| failed())
    }

    pub(super) async fn environment_show(&self, arguments: &Value) -> Result<Value, ToolError> {
        let store = anchor_library::EnvironmentStore::new(&self.library_root);
        // Reject a present-but-unusable argument instead of silently
        // falling back to the other one.
        for field in ["id", "name"] {
            if let Some(value) = arguments.get(field) {
                let usable = value.as_str().is_some_and(|text| !text.is_empty());
                if !usable {
                    return Err(rejected(format!(
                        "`{field}` must be a non-empty string when present"
                    )));
                }
            }
        }
        let id = match (
            arguments.get("id").and_then(Value::as_str),
            arguments.get("name").and_then(Value::as_str),
        ) {
            (Some(id), None) => id.to_owned(),
            (None, Some(name)) => store
                .names()
                .map_err(|_| failed())?
                .into_iter()
                .find(|entry| entry.name == name)
                .map(|entry| entry.id)
                .ok_or_else(|| rejected("no dependency environment has that name"))?,
            _ => return Err(rejected("pass exactly one of `id` or `name`")),
        };
        // `describe` re-hashes the published tree and reports whether it
        // is still writable, so an operator can inspect a store that
        // admission would refuse. The reason names the environment: a
        // bare "content is missing" tells nobody which one.
        let published = store.describe(&id).map_err(|error| {
            rejected(format!(
                "dependency environment `{id}` is not usable: {error}"
            ))
        })?;
        Ok(json!({
            "id":published.id,
            "bytes":published.bytes,
            "files":published.files,
            "directories":published.directories,
            "verified": !published.writable,
            "writable": published.writable,
        }))
    }
}
