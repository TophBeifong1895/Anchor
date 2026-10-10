#![forbid(unsafe_code)]

mod environment;
mod filesystem;
mod oauth;
mod review;
mod source;

use std::path::{Path, PathBuf};

use anchor_graph_host::FilePluginCatalog;
use serde::{Deserialize, Serialize};

pub use environment::{
    CandidateContent, EnvironmentError, EnvironmentStore, NameEntry, NameSwitch, PublishOutcome,
    PublishedEnvironment,
};
pub use oauth::{
    FileOAuthTokenStore, OAuthAuthorization, OAuthAuthorizationCode, OAuthAuthorizationMetadata,
    OAuthAuthorizationResponse, OAuthAuthorizationTransaction, OAuthBinding, OAuthClient,
    OAuthError, OAuthRefreshRequest, OAuthRefreshTransport, OAuthSecret, OAuthTokenResponse,
    OAuthTransportError,
};
pub use review::{ChannelReview, PluginReview, ServerReview};
pub use source::{Checkout, GitCheckout, GithubSource};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    pub source: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub replace_existing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallOutcome {
    pub id: String,
    pub digest: String,
    pub directory: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("source must be an HTTPS github.com tree URL identifying a Plugin directory")]
    InvalidSource,
    #[error("invalid Plugin ID")]
    InvalidId,
    #[error("Plugin already exists; explicitly request replacement to update it")]
    AlreadyExists,
    #[error("Plugin catalog is being installed; retry after installation finishes")]
    CatalogBusy,
    #[error("Plugin installation paths must contain only real directories and regular files")]
    UnsafePath,
    #[error("Plugin source has no plugin.json or .codex-plugin/plugin.json manifest")]
    MissingManifest,
    #[error("Plugin manifest or resources failed catalog validation")]
    InvalidPlugin,
    #[error("Plugin manifest or resources failed catalog validation: {0}")]
    InvalidPluginReason(String),
    #[error("Git checkout failed")]
    CheckoutFailed,
    #[error("Git checkout timed out")]
    CheckoutTimedOut,
    #[error("Plugin installation filesystem operation failed")]
    Io(#[source] std::io::Error),
    #[error("Plugin publication durability is uncertain; inspect the Library before retrying")]
    PublicationUncertain,
}

impl From<std::io::Error> for InstallError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone)]
pub struct Library {
    root: PathBuf,
}

impl Library {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn install(&self, request: &InstallRequest) -> Result<InstallOutcome, InstallError> {
        self.install_with_checkout(request, &GitCheckout)
    }

    pub fn catalog_read_guard(&self) -> Result<Option<std::fs::File>, InstallError> {
        match filesystem::directory(&self.root.join("plugins"), false) {
            Ok((_, directory)) => filesystem::lease(
                &directory,
                rustix::fs::FlockOperation::NonBlockingLockShared,
            )
            .map(Some),
            Err(InstallError::Io(failure)) if failure.kind() == std::io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(failure) => Err(failure),
        }
    }

    pub fn install_with_checkout(
        &self,
        request: &InstallRequest,
        checkout: &dyn Checkout,
    ) -> Result<InstallOutcome, InstallError> {
        let source = GithubSource::parse(&request.source)?;
        let id = request.id.as_deref().unwrap_or_else(|| source.default_id());
        validate_id(id)?;
        self.install_source(id, request.replace_existing, |transaction| {
            let directory = transaction.join("source");
            checkout.checkout(&source, &directory)?;
            Ok(directory.join(source.relative_path()))
        })
    }

    pub fn install_directory(
        &self,
        id: &str,
        source_dir: impl AsRef<Path>,
        replace_existing: bool,
    ) -> Result<InstallOutcome, InstallError> {
        validate_id(id)?;
        let source_dir = source_dir.as_ref().to_owned();
        self.install_source(id, replace_existing, |_| Ok(source_dir))
    }

    /// Describe a candidate Plugin without publishing it.
    ///
    /// The candidate is staged and validated through the same catalog an
    /// installation uses, so `PluginReview::digest` is the digest the installed
    /// Plugin will carry. A review never writes to the Library: it stages in the
    /// system temporary directory and drops the tree, which is why it also works
    /// when the Library is read-only for the reviewer.
    pub fn review_directory(
        &self,
        id: &str,
        source_dir: impl AsRef<Path>,
    ) -> Result<PluginReview, InstallError> {
        validate_id(id)?;
        let source_dir = source_dir.as_ref().to_owned();
        self.review_candidate(id, &source_dir)
    }

    /// Review a GitHub tree source without publishing it.
    pub fn review(&self, request: &InstallRequest) -> Result<PluginReview, InstallError> {
        self.review_with_checkout(request, &GitCheckout)
    }

    pub fn review_with_checkout(
        &self,
        request: &InstallRequest,
        checkout: &dyn Checkout,
    ) -> Result<PluginReview, InstallError> {
        let source = GithubSource::parse(&request.source)?;
        let id = request.id.as_deref().unwrap_or_else(|| source.default_id());
        validate_id(id)?;
        let transaction = tempfile::tempdir()?;
        let directory = transaction.path().join("source");
        checkout.checkout(&source, &directory)?;
        self.review_candidate(id, &directory.join(source.relative_path()))
    }

    fn review_candidate(&self, id: &str, source_dir: &Path) -> Result<PluginReview, InstallError> {
        // A review stages outside the Library, so it never needs write access
        // to the installed Plugins.
        let transaction = tempfile::Builder::new()
            .prefix(".anchor-plugin-review-")
            .tempdir()?;
        let (validation_root, _plugins, _staged) =
            stage_candidate(id, source_dir, transaction.path())?;
        let catalog = FilePluginCatalog::new(&validation_root);
        let definition = catalog
            .definition(id)
            .map_err(|error| InstallError::InvalidPluginReason(error.to_string()))?;
        review::summarize(id, &catalog, &definition)
    }

    fn install_source(
        &self,
        id: &str,
        replace_existing: bool,
        prepare: impl FnOnce(&Path) -> Result<PathBuf, InstallError>,
    ) -> Result<InstallOutcome, InstallError> {
        let (root, _) = filesystem::directory(&self.root, true)?;
        let (plugins, plugins_file) = filesystem::directory(&root.join("plugins"), true)?;
        let _lease = filesystem::lease(&plugins_file, rustix::fs::FlockOperation::LockExclusive)?;
        let destination = plugins.join(id);
        let replacing = filesystem::existing_destination(&destination, replace_existing)?;
        let transaction = tempfile::Builder::new()
            .prefix(".anchor-plugin-")
            .tempdir_in(&plugins)?;
        let source_dir = prepare(transaction.path())?;
        let (validation_root, validation_plugins, staged) =
            stage_candidate(id, &source_dir, transaction.path())?;
        let definition = FilePluginCatalog::new(&validation_root)
            .definition(id)
            .map_err(|_| InstallError::InvalidPlugin)?;
        filesystem::sync_tree(&staged)?;
        validation_plugins.sync_all()?;
        std::fs::File::open(&validation_root)?.sync_all()?;
        std::fs::File::open(transaction.path())?.sync_all()?;
        plugins_file.sync_all()?;
        let result = filesystem::publish(
            &validation_plugins,
            &plugins_file,
            id,
            replacing,
            filesystem::sync_publication,
        );
        if matches!(result, Err(InstallError::PublicationUncertain)) {
            let _ = transaction.keep();
        }
        result?;
        Ok(InstallOutcome {
            id: id.to_owned(),
            digest: definition.digest,
            directory: destination,
        })
    }
}

/// Stage a candidate Plugin under `transaction` and return the validation root,
/// the staged `plugins` directory, and the staged tree.
///
/// The caller runs catalog validation so it can map a failure to its own error
/// contract. Installation and review stage through this single function, which
/// is what keeps a review's digest identical to the published one.
fn stage_candidate(
    id: &str,
    source_dir: &Path,
    transaction: &Path,
) -> Result<(PathBuf, std::fs::File, PathBuf), InstallError> {
    let validation_root = transaction.join("validation");
    let (_, validation_plugins) = filesystem::directory(&validation_root.join("plugins"), true)?;
    let staged = validation_root.join("plugins").join(id);
    filesystem::stage(source_dir, &staged)?;
    Ok((validation_root, validation_plugins, staged))
}

fn validate_id(id: &str) -> Result<(), InstallError> {
    if id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    {
        Ok(())
    } else {
        Err(InstallError::InvalidId)
    }
}
