//! The shell's half of the scene's sound.
//!
//! A shell contains no engine logic, but it does own the speakers: the seam
//! says *what* plays (`Music`, `Sound`), the shell decides *how* — music
//! loops until the next cue, voices and effects are one-shots, and a new
//! voice politely stops the old, which is how the original reads aloud.
//!
//! Silence is a valid outcome. No audio device, a codec the bytes do not
//! match — the play goes on quietly and the note on screen says what the
//! gold could not reach. Audio must never be the reason a game stops.

use std::io::Cursor;

use kintsugi_core::asset::Audio;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

/// An open audio device with two channels of intent: one for the score,
/// one for the spoken and struck sounds of the scene.
pub struct Player {
    // The stream must outlive every sink made from its handle.
    _stream: OutputStream,
    handle: OutputStreamHandle,
    music: Option<Sink>,
    sound: Option<Sink>,
}

impl Player {
    /// Open the default output device, or `None` — a headless machine, a
    /// busy device — and let the caller say the play will be silent.
    pub fn try_new() -> Option<Self> {
        match OutputStream::try_default() {
            Ok((stream, handle)) => Some(Self {
                _stream: stream,
                handle,
                music: None,
                sound: None,
            }),
            Err(_) => None,
        }
    }

    /// Set the score: `Some` loops until the next cue, `None` is the
    /// measured "stop the music" cue. Undecodable bytes leave the previous
    /// piece playing rather than punishing the scene with silence.
    pub fn music(&mut self, audio: Option<Audio>) {
        let Some(audio) = audio else {
            if let Some(sink) = self.music.take() {
                sink.stop();
            }
            return;
        };
        let Ok(source) = decode(audio) else {
            return;
        };
        let Ok(sink) = Sink::try_new(&self.handle) else {
            return;
        };
        sink.append(source.repeat_infinite());
        sink.play();
        if let Some(old) = self.music.replace(sink) {
            old.stop();
        }
    }

    /// Play a one-shot — a voice line, a chime — stopping the one still
    /// sounding: the original never lets two voices read at once.
    pub fn sound(&mut self, audio: Audio) {
        let Ok(source) = decode(audio) else {
            return;
        };
        let Ok(sink) = Sink::try_new(&self.handle) else {
            return;
        };
        sink.append(source);
        sink.play();
        if let Some(old) = self.sound.replace(sink) {
            old.stop();
        }
    }
}

/// Decode a sniffed container; the seam already rejected what it does not
/// recognize, so a failure here means the bytes lied about their magic.
fn decode(
    audio: Audio,
) -> std::result::Result<Decoder<Cursor<Vec<u8>>>, rodio::decoder::DecoderError> {
    Decoder::new(Cursor::new(audio.data))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny PCM wave: one channel, 8 kHz, a hundred samples of near-silence.
    fn wave_bytes() -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36u32 + 200).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&200u32.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0u8, 200));
        bytes
    }

    #[test]
    fn a_wave_decodes_and_a_lie_does_not() {
        assert!(decode(Audio::new(wave_bytes())).is_ok());
        assert!(decode(Audio::new(b"not audio at all".to_vec())).is_err());
    }

    #[test]
    fn a_machine_without_a_device_is_not_an_error() {
        // On a host with a device this plays nothing and returns; on CI it
        // must simply be None. Either way the game goes on.
        let player = Player::try_new();
        if let Some(mut player) = player {
            player.music(None); // silence is always playable
        }
    }
}
