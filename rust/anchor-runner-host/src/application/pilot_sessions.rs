//! Pilot-specific creation policy. Protocol adapters supply identity and title;
//! generic Sessions and channel Sessions keep their own creation contracts.
use anchor_platform_session::{CreateSession, Session, SessionError, SessionStore};

pub(crate) fn create(
    store: &SessionStore,
    owner: &str,
    mut request: CreateSession,
) -> Result<Session, SessionError> {
    if !request.graph.is_empty() || !request.channel.is_empty() {
        return Err(SessionError::Invalid(
            "Pilot Sessions cannot bind a Graph or channel".into(),
        ));
    }
    request
        .plugin_mounts
        .entry(crate::plugin_network::NETWORK_PLUGIN.to_owned())
        .or_insert(true);
    store.create(owner, request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pilot_defaults_are_persisted_without_changing_generic_sessions_or_explicit_policy() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
        let request = |id: &str| CreateSession {
            id: Some(id.into()),
            ..Default::default()
        };
        let pilot = create(&store, "local", request("pilot")).unwrap();
        assert_eq!(pilot.plugin_mounts.get("network"), Some(&true));
        assert_eq!(
            store.get("local", "pilot").unwrap().plugin_mounts,
            pilot.plugin_mounts
        );
        let mut disabled = request("disabled");
        disabled.plugin_mounts.insert("network".into(), false);
        assert_eq!(
            create(&store, "local", disabled)
                .unwrap()
                .plugin_mounts
                .get("network"),
            Some(&false)
        );
        assert!(
            store
                .create("local", request("generic"))
                .unwrap()
                .plugin_mounts
                .is_empty()
        );
    }
}
