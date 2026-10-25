//! Pilot adapter for the existing browser OAuth protocol and token store.
use super::*;
use crate::application::{
    ApplicationError,
    oauth_management::{OAuthManagementPort, PilotOAuthOrigin},
};
use crate::dependency_provision::DependencyAccess;
use crate::oauth_links::PilotOAuthLink;
use anchor_platform_session::{SessionStatus, TurnStatus};
use std::{future::Future, pin::Pin};

pub(crate) struct OAuthManagementAdapter {
    data: PathBuf,
    library: PathBuf,
    redirect_uri: String,
}

impl OAuthManagementAdapter {
    pub(crate) fn new(data: PathBuf, library: PathBuf, listen: &str) -> Self {
        let redirect_uri = std::env::var("ANCHOR_OAUTH_REDIRECT_URI").unwrap_or_else(|_| {
            let address = listen.parse::<std::net::SocketAddr>().ok();
            let port = address.map(|address| address.port()).unwrap_or(8077);
            format!("http://127.0.0.1:{port}{OAUTH_CALLBACK_PATH}")
        });
        Self {
            data,
            library,
            redirect_uri,
        }
    }
}

impl OAuthManagementPort for OAuthManagementAdapter {
    fn callback_uri(&self) -> &str {
        &self.redirect_uri
    }
    fn status(&self, owner: &str, plugin: &str, server: &str) -> Result<Value, ApplicationError> {
        let route = resolve_binding_at_root(&self.library, plugin, server, &binding_owner(owner))
            .map_err(|_| {
            ApplicationError::Invalid("installed Plugin OAuth target is unavailable".into())
        })?;
        let authorization = anchor_library::FileOAuthTokenStore::new(&self.library)
            .load(&route.binding)
            .map_err(|_| {
                ApplicationError::Storage("OAuth authorization state unavailable".into())
            })?;
        Ok(project_authorization(&authorization))
    }
    fn authorize<'a>(
        &'a self,
        origin: &'a PilotOAuthOrigin,
        plugin: &'a str,
        server: &'a str,
        access: &'a DependencyAccess,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ApplicationError>> + Send + 'a>> {
        Box::pin(async move {
            let route = resolve_binding_at_root(
                &self.library,
                plugin,
                server,
                &binding_owner(&origin.owner),
            )
            .map_err(|_| {
                ApplicationError::Invalid("installed Plugin OAuth target is unavailable".into())
            })?;
            let result = tokio::select! {
                result=begin_authorization_route(route,self.redirect_uri.clone(),Some((self.data.clone(),origin.clone())))=>result.map_err(|_|ApplicationError::Invalid("browser OAuth authorization could not be prepared; check the Plugin OAuth declaration and callback URL".into())),
                _=access.cancelled()=>Err(ApplicationError::Invalid("OAuth authorization preparation was interrupted".into())),
            }?;
            access
                .check()
                .map_err(|error| ApplicationError::Invalid(error.to_string()))?;
            Ok(result)
        })
    }
}

pub(super) fn register(
    data: &Path,
    origin: &PilotOAuthOrigin,
    route: &OAuthRouteBinding,
    authorization_url: &Url,
    expires_at: u64,
) -> Result<(), String> {
    let state = authorization_url
        .query_pairs()
        .find(|(name, _)| name == "state")
        .map(|(_, state)| state.into_owned())
        .ok_or("OAuth transaction has no state")?;
    let current = origin
        .sessions
        .get(&origin.owner, &origin.session.id)
        .map_err(|_| "OAuth source Session unavailable")?;
    if current.created_at != origin.session.created_at
        || current.status == SessionStatus::Archived
        || origin
            .sessions
            .get_turn(&origin.owner, &current.id, &origin.turn)
            .map_err(|_| "OAuth source Turn unavailable")?
            .status
            != TurnStatus::Running
    {
        return Err("OAuth source is no longer active".into());
    }
    crate::oauth_links::save(
        data,
        &state,
        &PilotOAuthLink {
            format: 1,
            owner: origin.owner.clone(),
            session: current.id,
            created_at: current.created_at.to_rfc3339(),
            turn: origin.turn.clone(),
            binding: route.binding.clone(),
            digest: route.digest.clone(),
            authorization_digest: route.authorization_digest.clone(),
            expires_at,
            configured: false,
            cancelled: false,
        },
    )
}

pub(super) fn validate_link(
    state: &ApiState,
    link: &PilotOAuthLink,
    binding: &OAuthBinding,
) -> Result<anchor_platform_session::Session, String> {
    if &link.binding != binding
        || link.expires_at < unix_time().map_err(|_| "OAuth time unavailable")?
        || link.configured
        || link.cancelled
    {
        return Err("OAuth source association is no longer pending".into());
    }
    let sessions = store(state).map_err(|_| "OAuth Session store unavailable")?;
    let session = sessions
        .get(&link.owner, &link.session)
        .map_err(|_| "OAuth source Session unavailable")?;
    let turn = sessions
        .get_turn(&link.owner, &link.session, &link.turn)
        .map_err(|_| "OAuth source Turn unavailable")?;
    if session.created_at.to_rfc3339() != link.created_at
        || session.status == SessionStatus::Archived
        || turn.status == TurnStatus::Stopped
        || crate::pilot_host::continuation_cancelled(
            &sessions,
            &link.owner,
            &link.session,
            &link.turn,
        )
        .map_err(|_| "OAuth source continuation unavailable")?
    {
        return Err("OAuth source Session was stopped, deleted, or recreated".into());
    }
    let route = resolve_binding_at_root(
        state.application.library_root(),
        binding.plugin(),
        binding.server(),
        binding.owner(),
    )
    .map_err(|_| "OAuth target Plugin unavailable")?;
    if route.binding != link.binding
        || route.digest != link.digest
        || route.authorization_digest != link.authorization_digest
    {
        return Err(
            "OAuth target Plugin changed while the browser authorization was pending".into(),
        );
    }
    Ok(session)
}

pub(super) async fn publish_and_continue(
    state: &ApiState,
    state_value: &str,
    grant: OAuthGrant,
    link: Option<PilotOAuthLink>,
) -> Result<(), OAuthError> {
    let library = state.application.library_root().to_path_buf();
    let Some(mut link) = link else {
        return persist_authorization(grant, library).await;
    };
    let session =
        validate_link(state, &link, &grant.binding).map_err(|_| OAuthError::InvalidResponse)?;
    // Token exchange happens before taking the control gate. Publication is a
    // short local operation sharing stop/deletion's existing boundary.
    let control = state
        .pilots
        .credential_submission_guard(&crate::pilot_host::scope(
            &state.data_root,
            &link.owner,
            &session,
        ))
        .await
        .map_err(|_| OAuthError::InvalidResponse)?;
    let catalog = state.application.graph_catalog_mutation_guard().await;
    let lease = state
        .application
        .plugin_reference_lease()
        .await
        .map_err(|_| OAuthError::InvalidResponse)?;
    validate_link(state, &link, &grant.binding).map_err(|_| OAuthError::InvalidResponse)?;
    persist_authorization(grant, library).await?;
    link.configured = true;
    crate::oauth_links::save(&state.data_root, state_value, &link)
        .map_err(|_| OAuthError::Storage)?;
    drop(lease);
    drop(catalog);
    drop(control);
    let sessions = store(state).map_err(|_| OAuthError::Storage)?;
    let tools = crate::pilot_tools::PilotTools {
        application: state.application.clone(),
        graph_name: state.graph_name.clone(),
        library_root: state.application.library_root().to_path_buf(),
        data_root: state.data_root.clone(),
        workspace_root: state.workspace_root.clone(),
        sessions: sessions.clone(),
        owner: link.owner.clone(),
        session: session.id.clone(),
        turn: String::new(),
        context: None,
    };
    let admission = crate::pilot_host::PilotAdmission {
        sessions,
        owner: link.owner.clone(),
        session: session.clone(),
        request_id: String::new(),
        prompt: None,
        attachments: Default::default(),
        input_source: None,
        root: crate::pilot_host::scope(&state.data_root, &link.owner, &session),
        tools,
    };
    if state
        .pilots
        .ready_to_continue(
            admission,
            &link.turn,
            "owner-bound browser OAuth configured",
        )
        .await
        .is_err()
    {
        eprintln!("Pilot OAuth configured; source Turn cannot be automatically continued");
    }
    Ok(())
}
