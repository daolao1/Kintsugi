//! 🏺 Glaze layer №1: high-fidelity scaling and frame interpolation.
//!
//! 釉 (*uwagusuri*) is fired over the repaired vessel — it works on any
//! body, hides no seam, and serves every engine equally. This crate treats
//! frames the same way: [`UpscaleMethod::Anime4K`] sharpens and scales
//! line-art the way the Anime4K shaders made famous, and
//! [`FrameInterpolator`] leaves room for motion-compensated (RIFE-style)
//! backends to come.
//!
//! Everything here is dependency-free CPU math over
//! [`kintsugi_core::asset::Image`] frames, so any host — terminal, GUI, or
//! headless transcoder — can glaze any engine's output.
//!
//! ## On "Anime4K"
//!
//! [Anime4K](https://github.com/bloc97/Anime4K) by bloc97 (MIT) showed that
//! anime line-art upscales best with *edge-adaptive* filtering instead of
//! generic photo scalers. Its v2+ shaders are tiny CNNs; running those needs
//! a neural runtime, which the glaze refuses to grow. The preset here
//! follows the **classic v0.9 recipe** instead — a smooth high-quality base
//! upscale plus an unsharp mask whose strength adapts to local edge energy,
//! so flat cel areas stay clean while lines get their crispness back. It is
//! Anime4K-*inspired*, and says so honestly everywhere it is named.

pub mod interp;
pub mod upscale;

pub use interp::{
    BlendInterpolator, FrameInterpolator, NamedFrame, NotAFrame, group_by_size,
    interpolate_sequence,
};
pub use upscale::{UpscaleMethod, upscale};
