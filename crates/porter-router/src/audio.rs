//! The audio rules every `Transcribe` turn obeys: which frame may follow which
//! ([`check_audio`]), how long audio is ([`audio_ms`]) and where a turn reads its frames from
//! ([`AudioIn`]). The session machine and the audit tally use them; the speech runners that connect
//! a turn to a speech engine are the daemon's.
//!
//! Nothing keeps audio after a turn: frames live in the buffer of the running engine call and
//! are dropped with it, on `Cancel` too.

use crate::session::AudioCursor;
use porter_infer::{AudioFrame, AudioRate, ModelError};
use std::future::Future;

/// The longest audio one frame may carry, in milliseconds.
pub const MAX_FRAME_MS: u32 = 1_000;

/// Bytes per sample: signed 16-bit little-endian mono.
const BYTES_PER_SAMPLE: usize = 2;

/// The next audio cursor after `frame`, or why the frame is refused: it must start where the
/// last one ended, hold whole samples, be non-empty and carry at most one second.
pub fn check_audio(
    cursor: AudioCursor,
    rate: AudioRate,
    frame: &AudioFrame,
) -> Result<AudioCursor, ModelError> {
    let AudioCursor::Expecting { next } = cursor else {
        return Err(ModelError::Unreadable);
    };
    let bytes = frame.pcm.0.len();
    let samples = (bytes / BYTES_PER_SAMPLE) as u64;
    let whole = bytes.is_multiple_of(BYTES_PER_SAMPLE) && samples > 0;
    let short_enough = samples <= u64::from(rate.0) * u64::from(MAX_FRAME_MS) / 1_000;
    if whole && short_enough && frame.at == next {
        Ok(AudioCursor::Expecting {
            next: next + samples,
        })
    } else {
        Err(ModelError::Unreadable)
    }
}

/// Whole milliseconds of audio in `samples` at `rate` (zero for a zero rate).
pub fn audio_ms(rate: AudioRate, samples: u64) -> u32 {
    let ms = samples.saturating_mul(1_000).checked_div(u64::from(rate.0));
    ms.map_or(0, |ms| u32::try_from(ms).unwrap_or(u32::MAX))
}

/// What the audio source of a turn yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioPull {
    /// A frame, already checked by [`check_audio`].
    Frame(AudioFrame),
    /// `EndOfAudio` arrived.
    End,
}

/// Where a `Transcribe` turn reads its audio (the fd server, or a fake in tests).
pub trait AudioIn: Send {
    /// The next frame or the end.
    fn next(&mut self) -> impl Future<Output = AudioPull> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_infer::Base64Bytes;

    fn frame(at: u64, samples: usize) -> AudioFrame {
        AudioFrame {
            at,
            pcm: Base64Bytes(vec![0; samples * 2]),
        }
    }

    #[test]
    fn audio_frames_are_checked_in_order_and_at_most_a_second() {
        let rate = AudioRate(16_000);
        let at = |next| AudioCursor::Expecting { next };
        let cases = [
            ("the first frame", at(0), frame(0, 512), Ok(at(512))),
            (
                "the next frame",
                at(512),
                frame(512, 16_000),
                Ok(at(16_512)),
            ),
            ("exactly a second", at(0), frame(0, 16_000), Ok(at(16_000))),
            (
                "over a second",
                at(0),
                frame(0, 16_001),
                Err(ModelError::Unreadable),
            ),
            (
                "a gap",
                at(512),
                frame(1_000, 8),
                Err(ModelError::Unreadable),
            ),
            (
                "a repeat",
                at(512),
                frame(0, 8),
                Err(ModelError::Unreadable),
            ),
            ("empty", at(0), frame(0, 0), Err(ModelError::Unreadable)),
            (
                "an odd byte count",
                at(0),
                AudioFrame {
                    at: 0,
                    pcm: Base64Bytes(vec![0; 3]),
                },
                Err(ModelError::Unreadable),
            ),
            (
                "outside a transcribe turn",
                AudioCursor::NoAudio,
                frame(0, 8),
                Err(ModelError::Unreadable),
            ),
        ];
        for (name, cursor, f, expected) in cases {
            assert_eq!(check_audio(cursor, rate, &f), expected, "{name}");
        }
    }

    #[test]
    fn audio_milliseconds_follow_the_rate() {
        let cases = [
            (16_000, 16_000, 1_000),
            (16_000, 512, 32),
            (24_000, 24_000, 1_000),
            (16_000, 0, 0),
            (0, 100, 0),
        ];
        for (rate, samples, ms) in cases {
            assert_eq!(
                audio_ms(AudioRate(rate), samples),
                ms,
                "{rate} Hz, {samples}"
            );
        }
    }
}
