use std::path::PathBuf;

/// All errors this crate can produce.
#[derive(Debug, thiserror::Error)]
pub enum DatapackError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// JSON parse failure. `location` is `line L column C` from serde_json — not
    /// a full RFC 6901 JSON pointer (that needs `serde_path_to_error`, which is
    /// outside this crate's approved dependency list), but it pinpoints the
    /// failing token, which is what `#[serde(untagged)]`'s useless top-level
    /// error message otherwise loses.
    #[error("failed to parse {path} at {location}: {source}")]
    Json {
        path: PathBuf,
        location: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid resource location '{0}' in {1}")]
    InvalidResourceLocation(String, PathBuf),

    #[error("{registry}: {referrer} references unknown {kind} '{target}'")]
    DanglingReference {
        registry: &'static str,
        referrer: String,
        kind: &'static str,
        target: String,
    },

    #[error("{registry}: reference cycle detected: {cycle}")]
    Cycle {
        registry: &'static str,
        cycle: String,
    },

    #[error("{registry}: duplicate entry '{id}' (from {first} and {second})")]
    Duplicate {
        registry: &'static str,
        id: String,
        first: PathBuf,
        second: PathBuf,
    },

    #[error("no DataVersion found: neither {version_json} nor {pack_mcmeta} supplied one")]
    MissingDataVersion {
        version_json: PathBuf,
        pack_mcmeta: PathBuf,
    },

    #[error("pack root {0} has no `data` directory")]
    NotADatapack(PathBuf),
}

pub type Result<T> = std::result::Result<T, DatapackError>;

/// Deserialize `bytes` (already read from `path`) as JSON, wrapping any error
/// with the file path and a line/column location.
pub fn parse_json<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|source| DatapackError::Json {
        path: path.to_path_buf(),
        location: format!("line {} column {}", source.line(), source.column()),
        source,
    })
}
