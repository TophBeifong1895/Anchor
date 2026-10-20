//! Schedule management owns durable timing facts through a Host adapter. Graph
//! execution and permissions stay in the existing admission/ticker path.
use super::{ApplicationError, RunApplication};
use serde_json::Value;

pub(crate) trait ScheduleManagementPort: Send + Sync {
    fn list(&self) -> Result<Value, ApplicationError>;
    fn create(&self, graph: &str, rule: &Value, input: &Value) -> Result<Value, ApplicationError>;
    fn replay_creation(
        &self,
        _owner: &str,
        _key: &str,
        _content: &Value,
    ) -> Result<Option<Value>, ApplicationError> {
        Err(ApplicationError::Invalid(
            "keyed schedule creation is not supported by this Host".into(),
        ))
    }
    fn create_with_request(
        &self,
        graph: &str,
        rule: &Value,
        input: &Value,
        _owner: &str,
        key: Option<&str>,
    ) -> Result<Value, ApplicationError> {
        if key.is_some() {
            return Err(ApplicationError::Invalid(
                "keyed schedule creation is not supported by this Host".into(),
            ));
        }
        self.create(graph, rule, input)
    }
    fn update(
        &self,
        id: &str,
        graph: &str,
        rule: &Value,
        input: &Value,
        enabled: bool,
    ) -> Result<Value, ApplicationError>;
    fn delete(&self, id: &str) -> Result<Value, ApplicationError>;
}

impl RunApplication {
    fn schedule_management(&self) -> Result<&dyn ScheduleManagementPort, ApplicationError> {
        self.schedules.as_deref().ok_or_else(|| {
            ApplicationError::Invalid("this Host does not provide schedule management".into())
        })
    }

    pub(crate) fn list_schedules(&self) -> Result<Value, ApplicationError> {
        self.schedule_management()?.list()
    }

    #[cfg(test)]
    pub(crate) async fn create_schedule(
        &self,
        graph: &str,
        rule: &Value,
        input: &Value,
    ) -> Result<Value, ApplicationError> {
        self.create_schedule_with_request(graph, rule, input, "local", None)
            .await
    }

    pub(crate) async fn create_schedule_with_request(
        &self,
        graph: &str,
        rule: &Value,
        input: &Value,
        owner: &str,
        key: Option<&str>,
    ) -> Result<Value, ApplicationError> {
        let port = self.schedule_management()?;
        if graph.trim().is_empty() {
            return Err(ApplicationError::Invalid("graph is required".into()));
        }
        if !rule.is_object() || !input.is_object() {
            return Err(ApplicationError::Invalid(
                "rule and input must be objects".into(),
            ));
        }
        // Graph deletion/publication and scheduling share the same guard; a
        // schedule cannot be created against a graph removed during validation.
        let _catalog_guard = self.graph_catalog_mutation_guard().await;
        if let Some(key) = key {
            super::pilot_sessions::validate_request_id(key)
                .map_err(|failure| ApplicationError::Invalid(failure.to_string()))?;
            if let Some(retained) = port.replay_creation(
                owner,
                key,
                &serde_json::json!({"graph":graph,"rule":rule,"input":input}),
            )? {
                return Ok(retained);
            }
        }
        self.load_graph(graph)
            .map_err(|_| ApplicationError::Missing)?;
        port.create_with_request(graph, rule, input, owner, key)
    }

    pub(crate) fn delete_schedule(&self, id: &str) -> Result<Value, ApplicationError> {
        self.schedule_management()?.delete(id)
    }

    pub(crate) async fn update_schedule(
        &self,
        id: &str,
        graph: &str,
        rule: &Value,
        input: &Value,
        enabled: bool,
    ) -> Result<Value, ApplicationError> {
        let port = self.schedule_management()?;
        if graph.trim().is_empty() || !rule.is_object() || !input.is_object() {
            return Err(ApplicationError::Invalid(
                "graph, object rule and object input are required".into(),
            ));
        }
        let _catalog_guard = self.graph_catalog_mutation_guard().await;
        self.load_graph(graph)
            .map_err(|_| ApplicationError::Missing)?;
        port.update(id, graph, rule, input, enabled)
    }
}
