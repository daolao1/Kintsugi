//! Everything a seam or host usually needs, in one `use`.
//!
//! ```
//! use kintsugi_core::prelude::*;
//! let path = VirtualPath::new("DATA\\Title.ZBM");
//! assert_eq!(path.as_str(), "data/title.zbm");
//! ```

pub use crate::asset::{Audio, AudioCodec, Image, ImageFormat};
pub use crate::bytes::{MsbBitReader, Reader};
pub use crate::codec::bmp::decode_bmp;
pub use crate::codec::png::encode_png;
pub use crate::detect::{Confidence, Detection};
pub use crate::error::{Error, Result};
pub use crate::iso::{IsoSource, Naming};
pub use crate::plugin::{EngineMount, EnginePlugin, MountInfo, PluginMetadata, Registry};
pub use crate::runtime::{Event, Host, Interpreter, STEP_BUDGET};
pub use crate::script::{ChoiceOption, Command, Script};
pub use crate::vfs::{DirectorySource, FileSource, Vfs, VirtualPath};
