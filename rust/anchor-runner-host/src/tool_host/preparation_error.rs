//! Preserve known preparation refusals without relabeling unavailable state.

#[derive(Debug)]
pub(crate) enum PreparationError {
    /// A completed configuration, authorization, or protocol check failed.
    KnownFailure(String),
    /// Preparation did not establish a known result, including I/O and cleanup.
    Unavailable(String),
}

impl std::fmt::Display for PreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KnownFailure(reason) | Self::Unavailable(reason) => formatter.write_str(reason),
        }
    }
}

impl From<String> for PreparationError {
    fn from(reason: String) -> Self {
        Self::Unavailable(reason)
    }
}

impl PreparationError {
    pub(super) fn context(self, prefix: &str) -> Self {
        match self {
            Self::KnownFailure(reason) => Self::KnownFailure(format!("{prefix}: {reason}")),
            Self::Unavailable(reason) => Self::Unavailable(format!("{prefix}: {reason}")),
        }
    }

    /// Failed cleanup prevents claiming that preparation has a known outcome.
    pub(super) fn cleanup(self, cleanup: Option<String>) -> Self {
        match cleanup {
            Some(cleanup) => Self::Unavailable(format!("{self}; cleanup failed: {cleanup}")),
            None => self,
        }
    }
}
