//! Browser OAuth stays in the existing Host adapter; Pilot receives only a URL
//! and authorization metadata, never token values.
use super::{ApplicationError, RunApplication};
use crate::dependency_provision::DependencyAccess;
use anchor_platform_session::{Session, SessionStore, TurnStatus};
use serde_json::Value;
use std::{future::Future, pin::Pin};

#[derive(Clone)]
pub(crate) struct PilotOAuthOrigin {
    pub(crate) sessions: SessionStore,
    pub(crate) owner: String,
    pub(crate) session: Session,
    pub(crate) turn: String,
}

pub(crate) trait OAuthManagementPort: Send + Sync {
    fn callback_uri(&self) -> &str;
    fn status(&self, owner: &str, plugin: &str, server: &str) -> Result<Value, ApplicationError>;
    fn authorize<'a>(
        &'a self,
        origin: &'a PilotOAuthOrigin,
        plugin: &'a str,
        server: &'a str,
        access: &'a DependencyAccess,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ApplicationError>> + Send + 'a>>;
}

impl RunApplication {
    pub(crate) fn oauth_callback_uri(&self) -> Option<&str> {
        self.oauth.as_deref().map(OAuthManagementPort::callback_uri)
    }
    pub(crate) fn oauth_status(
        &self,
        owner: &str,
        plugin: &str,
        server: &str,
    ) -> Result<Value, ApplicationError> {
        self.oauth
            .as_ref()
            .ok_or_else(|| {
                ApplicationError::Invalid("this Host has no browser OAuth adapter".into())
            })?
            .status(owner, plugin, server)
    }

    pub(crate) async fn authorize_pilot_oauth(
        &self,
        origin: &PilotOAuthOrigin,
        plugin: &str,
        server: &str,
        access: &DependencyAccess,
    ) -> Result<Value, ApplicationError> {
        access
            .check()
            .map_err(|error| ApplicationError::Invalid(error.to_string()))?;
        if !access.network {
            return Err(ApplicationError::Invalid(
                "OAuth authorization requires this Turn's task network authorization".into(),
            ));
        }
        if origin
            .sessions
            .get_turn(&origin.owner, &origin.session.id, &origin.turn)
            .map_err(|error| ApplicationError::Invalid(error.to_string()))?
            .status
            != TurnStatus::Running
        {
            return Err(ApplicationError::Invalid(
                "OAuth authorization requires an owned Running Turn".into(),
            ));
        }
        self.oauth
            .as_ref()
            .ok_or_else(|| {
                ApplicationError::Invalid("this Host has no browser OAuth adapter".into())
            })?
            .authorize(origin, plugin, server, access)
            .await
    }
}
