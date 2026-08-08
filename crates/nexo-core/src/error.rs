use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("failed to read/write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("io error: {0}")]
    RawIo(#[from] std::io::Error),

    #[error("malformed JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    /// The user asked for something that can't work — wrong loader, missing
    /// Java, an instance that doesn't exist. Safe to show verbatim in the UI.
    #[error("{0}")]
    Invalid(String),

    #[error("sign-in failed: {0}")]
    Auth(String),

    #[error("could not locate a home/data directory for the current user")]
    NoDataDir,
}

impl Error {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    pub fn auth(msg: impl Into<String>) -> Self {
        Self::Auth(msg.into())
    }
}

/// Attaches the offending path to an [`std::io::Error`], since bare io errors
/// ("permission denied") are nearly useless in a launcher that touches
/// hundreds of files per install.
pub(crate) trait IoContext<T> {
    fn ctx(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::result::Result<T, std::io::Error> {
    fn ctx(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.into(),
            source,
        })
    }
}
