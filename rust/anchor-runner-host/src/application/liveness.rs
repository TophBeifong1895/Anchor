//! Tests for the shared Run facade's liveness observation.
#[cfg(test)]
mod tests {
    use crate::application::{RunApplication, run_access::fixtures};

    #[test]
    fn parked_or_unregistered_executing_runs_are_not_quiescent() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
        for id in fixtures::scenarios(&application) {
            let view = fixtures::view(&application, id);
            assert_eq!(
                view.is_quiescent(),
                !matches!(id, "executing" | "child" | "recovery"),
                "{id}"
            );
            assert!(!view.with_active(true).is_quiescent(), "{id} with a worker");
        }
    }
}
