//! Schedule management owns durable timing facts through a Host adapter. Graph
//! execution and permissions stay in the existing admission/ticker path.
use super::{ApplicationError, RunApplication};
use serde_json::Value;

pub(crate) trait ScheduleManagementPort: Send + Sync {
    fn list(&self) -> Result<Value, ApplicationError>;
    fn create(&self, graph: &str, rule: &Value, input: &Value) -> Result<Value, ApplicationError>;
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

    pub(crate) async fn create_schedule(
        &self,
        graph: &str,
        rule: &Value,
        input: &Value,
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
        self.load_graph(graph)
            .map_err(|_| ApplicationError::Missing)?;
        port.create(graph, rule, input)
    }

    pub(crate) fn delete_schedule(&self, id: &str) -> Result<Value, ApplicationError> {
        self.schedule_management()?.delete(id)
    }
}
