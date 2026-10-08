//! Decoded assets: images and audio in the body's canonical forms.

/// Pixel layout of a decoded [`Image`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageFormat {
    /// 8 bits per channel, red-green-blue-alpha, tightly packed.
    Rgba8,
}

/// A decoded image.
#[derive(Clone, Debug)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel layout (currently always [`ImageFormat::Rgba8`]).
    pub format: ImageFormat,
    /// Pixel data, row-major, top-down.
    pub data: Vec<u8>,
}

impl Image {
    /// A fully transparent black image.
    ///
    /// Panics if `width * height * 4` overflows; decoders validate dimensions
    /// before calling this.
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as u64)
            .checked_mul(height as u64)
            .and_then(|px| px.checked_mul(4))
            .and_then(|len| usize::try_from(len).ok())
            .unwrap_or_else(|| panic!("image {width}x{height} overflows memory"));
        Self {
            width,
            height,
            format: ImageFormat::Rgba8,
            data: vec![0u8; len],
        }
    }

    /// Pixel bytes.
    pub fn rgba(&self) -> &[u8] {
        &self.data
    }

    /// Consume the image, returning its pixel bytes.
    pub fn into_rgba(self) -> Vec<u8> {
        self.data
    }
}

/// Container/codec of a still-undecoded audio asset.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AudioCodec {
    /// Ogg (usually Vorbis).
    Ogg,
    /// RIFF WAVE.
    Wav,
    /// Sniffing failed; bytes are passed through untouched.
    Unknown,
}

/// A still-undecoded audio asset.
///
/// The body hands audio through as containers for now: mixing is a host
/// concern, and the seam's job is only to get the bytes out of the archive
/// un-mangled. Playback is future gold, documented in the roadmap.
#[derive(Clone, Debug)]
pub struct Audio {
    /// Sniffed container.
    pub codec: AudioCodec,
    /// Raw container bytes.
    pub data: Vec<u8>,
}

impl Audio {
    /// Wrap raw audio bytes, sniffing the container from its magic.
    pub fn new(data: Vec<u8>) -> Self {
        let codec = Self::sniff(&data);
        Self { codec, data }
    }

    fn sniff(data: &[u8]) -> AudioCodec {
        if data.starts_with(b"OggS") {
            AudioCodec::Ogg
        } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
            AudioCodec::Wav
        } else {
            AudioCodec::Unknown
        }
    }
}
