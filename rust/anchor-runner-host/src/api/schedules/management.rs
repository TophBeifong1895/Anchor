//! The shared schedule mutation adapter and its HTTP entry points.
use super::*;

/// Both HTTP and Pilot use this adapter over the same Host-owned store. The
/// adapter adds no execution grant and never starts a Run while creating a plan.
pub(in crate::api) struct ScheduleManagementAdapter {
    store: ScheduleStoreHandle,
}

impl ScheduleManagementAdapter {
    pub(in crate::api) fn new(store: ScheduleStoreHandle) -> Self {
        Self { store }
    }
}

impl crate::application::scheduling::ScheduleManagementPort for ScheduleManagementAdapter {
    fn list(&self) -> Result<Value, ApplicationError> {
        let store = self
            .store
            .lock()
            .map_err(|_| schedule_store_unavailable())?;
        Ok(json!({"schedules": &store.items}))
    }

    fn create(&self, graph: &str, rule: &Value, input: &Value) -> Result<Value, ApplicationError> {
        if graph.trim().is_empty() {
            return Err(ApplicationError::Invalid("graph is required".into()));
        }
        if !input.is_object() {
            return Err(ApplicationError::Invalid("input must be an object".into()));
        }
        let precise_now = Local::now().naive_local();
        let normalized_rule =
            normalize_rule(rule, Some(precise_now)).map_err(ApplicationError::Invalid)?;
        let next_at =
            next_after(&normalized_rule, precise_now).map_err(ApplicationError::Invalid)?;
        let created_at = precise_now
            .with_nanosecond(0)
            .expect("zero nanoseconds is valid");
        let item = ScheduleItem {
            id: unique_schedule_id(),
            graph: graph.into(),
            rule: normalized_rule,
            input: input.clone(),
            created_at: format_local_datetime(created_at),
            next_at: format_local_datetime(next_at),
            enabled: true,
        };
        let mut store = self
            .store
            .lock()
            .map_err(|_| schedule_store_unavailable())?;
        let previous = store.items.clone();
        store.items.push(item.clone());
        if let Err(message) = store.persist() {
            store.items = previous;
            return Err(ApplicationError::Storage(message));
        }
        Ok(json!({"schedule": item}))
    }

    fn update(
        &self,
        id: &str,
        graph: &str,
        rule: &Value,
        input: &Value,
        enabled: bool,
    ) -> Result<Value, ApplicationError> {
        if graph.trim().is_empty() || !input.is_object() {
            return Err(ApplicationError::Invalid(
                "graph and object input are required".into(),
            ));
        }
        let normalized = normalize_rule(rule, None).map_err(ApplicationError::Invalid)?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| schedule_store_unavailable())?;
        let index = store
            .items
            .iter()
            .position(|item| item.id == id)
            .ok_or(ApplicationError::Missing)?;
        let previous = store.items[index].clone();
        let mut updated = previous.clone();
        updated.graph = graph.into();
        updated.rule = normalized;
        updated.input = input.clone();
        updated.enabled = enabled;
        // Editing only inputs preserves the next occurrence. Re-enabling or changing
        // the rule starts in the future; missed occurrences are never replayed.
        if enabled && (!previous.enabled || previous.rule != updated.rule) {
            let now = Local::now().naive_local();
            updated.rule =
                normalize_rule(&updated.rule, Some(now)).map_err(ApplicationError::Invalid)?;
            updated.next_at = format_local_datetime(
                next_after(&updated.rule, now).map_err(ApplicationError::Invalid)?,
            );
        }
        store.items[index] = updated.clone();
        if let Err(message) = store.persist() {
            store.items[index] = previous;
            return Err(ApplicationError::Storage(message));
        }
        Ok(json!({"schedule": updated}))
    }

    fn delete(&self, id: &str) -> Result<Value, ApplicationError> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| schedule_store_unavailable())?;
        let previous = store.items.clone();
        store.items.retain(|item| item.id != id);
        if store.items.len() == previous.len() {
            return Err(ApplicationError::Missing);
        }
        if let Err(message) = store.persist() {
            store.items = previous;
            return Err(ApplicationError::Storage(message));
        }
        Ok(json!({"schedule": id, "deleted": true}))
    }
}

fn schedule_store_unavailable() -> ApplicationError {
    ApplicationError::Storage("schedule store unavailable".into())
}

fn management_error(failure: ApplicationError, missing: &str) -> HttpResponse {
    match failure {
        ApplicationError::Missing => error(StatusCode::NOT_FOUND, missing),
        ApplicationError::Invalid(message) => error(StatusCode::BAD_REQUEST, message),
        ApplicationError::Conflict(message) => error(StatusCode::CONFLICT, message),
        ApplicationError::Storage(message) => error(StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}

pub(in crate::api) async fn list_schedules(
    State(state): State<ApiState>,
) -> Result<Json<Value>, HttpResponse> {
    state
        .application
        .list_schedules()
        .map(Json)
        .map_err(|failure| management_error(failure, "no such schedule"))
}

pub(in crate::api) async fn create_schedule(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), HttpResponse> {
    let Some(object) = body.as_object() else {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "provide graph, rule, and optional object input",
        ));
    };
    if object
        .keys()
        .any(|key| !["graph", "rule", "input"].contains(&key.as_str()))
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "unsupported schedule fields",
        ));
    }
    let graph = object
        .get("graph")
        .and_then(Value::as_str)
        .filter(|graph| !graph.trim().is_empty())
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "graph is required"))?;
    let input = object.get("input").cloned().unwrap_or_else(empty_object);
    if !input.is_object() {
        return Err(error(StatusCode::BAD_REQUEST, "input must be an object"));
    }
    let rule = object
        .get("rule")
        .filter(|rule| rule.is_object())
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "rule must be an object"))?;

    let created = state
        .application
        .create_schedule(graph, rule, &input)
        .await
        .map_err(|failure| management_error(failure, &format!("no such graph: {graph}")))?;
    Ok((StatusCode::CREATED, Json(created)))
}

pub(in crate::api) async fn delete_schedule(
    State(state): State<ApiState>,
    AxumPath(identifier): AxumPath<String>,
) -> Result<Json<Value>, HttpResponse> {
    state
        .application
        .delete_schedule(&identifier)
        .map(Json)
        .map_err(|failure| management_error(failure, "no such schedule"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api) struct ScheduleUpdate {
    graph: String,
    rule: Value,
    #[serde(default = "empty_object")]
    input: Value,
    enabled: bool,
}

pub(in crate::api) async fn update_schedule(
    State(state): State<ApiState>,
    AxumPath(identifier): AxumPath<String>,
    Json(body): Json<ScheduleUpdate>,
) -> Result<Json<Value>, HttpResponse> {
    state
        .application
        .update_schedule(
            &identifier,
            &body.graph,
            &body.rule,
            &body.input,
            body.enabled,
        )
        .await
        .map(Json)
        .map_err(|failure| management_error(failure, "no such Graph or schedule"))
}
