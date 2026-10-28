//! Framing for a session socket (design/31 §4.3): a 4-byte big-endian length, then one JSON
//! envelope carrying the vocabulary version and the body.

use crate::error::FrameError;
use porter_capability::VocabVersion;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The longest frame body, in bytes.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// What travels in a frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// The sender's vocabulary version.
    pub vocab: VocabVersion,
    /// The request or reply.
    pub body: T,
}

/// One frame for `body`, at this build's vocabulary version.
pub fn encode_frame<T: Serialize>(body: &T) -> Result<Vec<u8>, FrameError> {
    let envelope = Envelope {
        vocab: VocabVersion::CURRENT,
        body,
    };
    let json = serde_json::to_vec(&envelope).map_err(|e| FrameError::Malformed(e.to_string()))?;
    let len = u32::try_from(json.len())
        .ok()
        .filter(|len| (*len as usize) <= MAX_FRAME)
        .ok_or(FrameError::TooLong(json.len()))?;
    Ok(len.to_be_bytes().into_iter().chain(json).collect())
}

/// The first frame in `buffer` and how many bytes of the buffer it used, or `None` while the
/// buffer holds only part of one.
pub fn decode_frame<T: DeserializeOwned>(
    buffer: &[u8],
) -> Result<Option<(Envelope<T>, usize)>, FrameError> {
    let Some((head, rest)) = buffer.split_first_chunk::<4>() else {
        return Ok(None);
    };
    let len = u32::from_be_bytes(*head) as usize;
    if len > MAX_FRAME {
        return Err(FrameError::TooLong(len));
    }
    let Some(json) = rest.get(..len) else {
        return Ok(None);
    };
    let envelope =
        serde_json::from_slice(json).map_err(|e| FrameError::Malformed(e.to_string()))?;
    Ok(Some((envelope, 4 + len)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountsRequest, ParentWindow};
    use porter_core::ProviderHint;

    fn request() -> AccountsRequest {
        AccountsRequest::AddAccount {
            hint: ProviderHint::Any,
            window: ParentWindow::Unparented,
        }
    }

    #[test]
    fn a_frame_decodes_to_what_was_encoded() {
        let bytes = encode_frame(&request()).expect("encodes");
        let read = decode_frame::<AccountsRequest>(&bytes).expect("decodes");
        let expected = Envelope {
            vocab: VocabVersion::CURRENT,
            body: request(),
        };
        assert_eq!(read, Some((expected, bytes.len())));
    }

    #[test]
    fn a_short_buffer_is_partial() {
        let bytes = encode_frame(&request()).expect("encodes");
        for cut in [0, 3, bytes.len() - 1] {
            let read = decode_frame::<AccountsRequest>(&bytes[..cut]).expect("no error");
            assert_eq!(read, None, "cut at {cut}");
        }
    }

    #[test]
    fn an_oversized_length_is_refused() {
        let bytes = u32::MAX.to_be_bytes();
        let read = decode_frame::<AccountsRequest>(&bytes);
        assert_eq!(read, Err(FrameError::TooLong(u32::MAX as usize)));
    }
}
