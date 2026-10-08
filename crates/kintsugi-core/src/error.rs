//! The single error type shared by the body and every golden seam.

use std::fmt;

/// Result alias used throughout Kintsugi.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Every failure mode the engine reports.
///
/// Seams convert their format-specific failures into these variants so the
/// host and each other seam share one vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A path was not found in any mounted source.
    NotFound(String),
    /// Data claims to be a known format but violates its layout.
    Corrupt {
        /// Where the failure happened (format and/or file).
        context: String,
        /// What exactly was wrong.
        detail: String,
    },
    /// A valid question, but this build does not answer it.
    Unsupported {
        /// The format or feature being asked about.
        format: String,
        /// Why it is not supported here.
        detail: String,
    },
    /// Underlying storage failed.
    Io(String),
    /// A mounted script cannot be interpreted as-is.
    Script {
        /// Which script failed.
        context: String,
        /// What exactly was wrong.
        detail: String,
    },
    /// Text cannot be represented in the legacy code page being written to.
    ///
    /// Raised instead of quietly substituting look-alikes or HTML numeric
    /// character references: a repair must never alter the words it inserts.
    /// `encoding_rs` would happily turn an em dash into the literal text
    /// `&#8212;`, which the game would then render as garbage.
    Encoding {
        /// Which code page was asked to hold the text.
        encoding: String,
        /// The characters it cannot hold, in order of first appearance.
        characters: String,
    },
    /// Plugin-layer failure not covered by the variants above.
    Plugin(String),
}

impl Error {
    /// Build a [`Error::Corrupt`] in one call.
    pub fn corrupt(context: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Corrupt {
            context: context.into(),
            detail: detail.into(),
        }
    }

    /// Build an [`Error::Unsupported`] in one call.
    pub fn unsupported(format: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Unsupported {
            format: format.into(),
            detail: detail.into(),
        }
    }

    /// Build an [`Error::Encoding`] in one call.
    pub fn encoding(encoding: impl Into<String>, characters: impl Into<String>) -> Self {
        Self::Encoding {
            encoding: encoding.into(),
            characters: characters.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(what) => write!(f, "not found: {what}"),
            Error::Corrupt { context, detail } => write!(f, "corrupt {context}: {detail}"),
            Error::Unsupported { format, detail } => {
                write!(f, "unsupported {format}: {detail}")
            }
            Error::Io(detail) => write!(f, "i/o error: {detail}"),
            Error::Script { context, detail } => write!(f, "script {context}: {detail}"),
            Error::Encoding {
                encoding,
                characters,
            } => write!(
                f,
                "cannot write text as {encoding}: character(s) {characters} \
                 are not in that code page, and substituting them would \
                 change the game's words"
            ),
            Error::Plugin(detail) => write!(f, "plugin error: {detail}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound(e.to_string()),
            _ => Error::Io(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_readable() {
        let e = Error::corrupt("SNN archive", "entry 'title.zbm' exceeds file size");
        assert_eq!(
            e.to_string(),
            "corrupt SNN archive: entry 'title.zbm' exceeds file size"
        );
    }
}
