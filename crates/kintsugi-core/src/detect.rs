//! What an engine seam recognized, and how sure it is.
//!
//! Detection results are part of the repair's honesty: a seam must be able
//! to say "maybe" without the host treating that as a promise.

use std::fmt;

/// How strongly a seam believes the mounted files belong to its engine.
///
/// Ordered from weakest to strongest; `Certain` should require a verified
/// structural signature (a parsed index, a decoded header), never just a
/// file extension.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Confidence {
    /// Signatures absent or contradicted.
    Unlikely,
    /// Weak signals only (known extensions, loose magic matches).
    Possible,
    /// Strong signals (decoded headers, consistent structure).
    Likely,
    /// Verified structure (a complete index parsed, a header decoded).
    Certain,
}

impl Confidence {
    /// Human-readable label for reports.
    pub fn label(&self) -> &'static str {
        match self {
            Confidence::Unlikely => "unlikely",
            Confidence::Possible => "possible",
            Confidence::Likely => "likely",
            Confidence::Certain => "certain",
        }
    }
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One seam's verdict about the mounted game.
#[derive(Clone, Debug)]
pub struct Detection {
    /// Engine id, e.g. `"bluegale"`.
    pub engine: &'static str,
    /// How sure the seam is.
    pub confidence: Confidence,
    /// What was seen, in plain words — shown to the user as the visible gold.
    pub note: String,
}

impl Detection {
    /// Build a detection verdict.
    pub fn new(engine: &'static str, confidence: Confidence, note: impl Into<String>) -> Self {
        Self {
            engine,
            confidence,
            note: note.into(),
        }
    }
}
