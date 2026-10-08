//! 🏺 Kintsugi Engine — core runtime (the ceramic body).
//!
//! 金継ぎ (*kintsugi*): the Japanese art of repairing broken pottery with
//! gold. The cracks are not hidden — they become the most precious part of
//! the vessel. Kintsugi Engine applies that art to old games whose original
//! studios, source code, and platforms are gone:
//!
//! * the **body** — this crate: a small, dependency-free runtime with a
//!   virtual filesystem, format detection, a script IR and interpreter, and
//!   the codecs every seam shares (BMP in, PNG out);
//! * the **golden seams** — one crate per dead engine (`kintsugi-bluegale`,
//!   and one day `kintsugi-kid`, `kintsugi-…`), translating that engine's
//!   formats into the body's vocabulary.
//!
//! The seams are worn openly, the way kintsugi wears its gold: detection
//! reports *what* was recognized and *how confidently*; mounts record every
//! workaround they performed; bytes the community has not reverse-engineered
//! yet are surfaced as [`Command::RawLine`] instead of being silently
//! dropped. Preservation means honesty about the repair.

pub mod asset;
pub mod bytes;
pub mod codec;
pub mod detect;
pub mod error;
pub mod iso;
pub mod plugin;
pub mod prelude;
pub mod runtime;
pub mod script;
pub mod vfs;

pub use error::{Error, Result};
pub use iso::IsoSource;
pub use vfs::VirtualPath;
