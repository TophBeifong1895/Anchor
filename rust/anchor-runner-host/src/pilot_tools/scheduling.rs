//! Pilot scheduling is a projection of the existing Host scheduling use cases.
use super::*;

impl PilotTools {
    pub(super) async fn schedule_list(&self, _arguments: &Value) -> Result<Value, ToolError> {
        self.application.list_schedules().map_err(schedule_error)
    }

    pub(super) async fn schedule_create(&self, arguments: &Value) -> Result<Value, ToolError> {
        self.require_active_turn()?;
        let graph = parameter(arguments, "graph")?;
        let rule = arguments
            .get("rule")
            .filter(|rule| rule.is_object())
            .ok_or_else(|| rejected("rule must be an object"))?;
        let input = arguments.get("input").cloned().unwrap_or_else(|| json!({}));
        if !input.is_object() {
            return Err(rejected("input must be an object"));
        }
        if arguments.as_object().is_none_or(|object| {
            object
                .keys()
                .any(|key| !["graph", "rule", "input"].contains(&key.as_str()))
        }) {
            return Err(rejected("unsupported schedule fields"));
        }
        self.application
            .create_schedule(&graph, rule, &input)
            .await
            .map_err(schedule_error)
    }

    pub(super) async fn schedule_delete(&self, arguments: &Value) -> Result<Value, ToolError> {
        self.require_active_turn()?;
        let id = parameter(arguments, "schedule")?;
        if arguments
            .as_object()
            .is_none_or(|object| object.keys().any(|key| key != "schedule"))
        {
            return Err(rejected("unsupported schedule deletion fields"));
        }
        self.application
            .delete_schedule(&id)
            .map_err(schedule_error)
    }
}

fn schedule_error(failure: crate::application::ApplicationError) -> ToolError {
    match failure {
        crate::application::ApplicationError::Missing => rejected("no such Graph or schedule"),
        crate::application::ApplicationError::Invalid(reason)
        | crate::application::ApplicationError::Conflict(reason)
        | crate::application::ApplicationError::Storage(reason) => rejected(reason),
    }
}

#[cfg(test)]
mod tests;
