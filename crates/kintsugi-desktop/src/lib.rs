//! The desktop shell: a window that renders what an engine mount decodes,
//! and a headless twin that writes the same frames to PNG so the shell can
//! be verified without a screen.
//!
//! Rule four, honoured: there is no engine logic here. Everything the seam
//! knows arrives through `Host` calls; this crate owns pixels, fonts, input
//! and timing, never engine formats.

pub mod audio;
pub mod font;
pub mod glaze;
pub mod render;
pub mod state;
