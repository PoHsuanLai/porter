//! The Photos Library API calls an upload makes: bytes to an upload token, an album, and the
//! token made into a media item. Pure over any [`Http`]; in syncd the `Http` is a relay that adds
//! the bearer.

use porter_http::http::request::Builder;
use porter_http::http::{Method, Response, StatusCode};
use porter_http::{HeaderText, Http, request};
use porter_ids::{UnixSeconds, WebUrl};
use porter_sync::Clock;
use serde::Deserialize;

/// What an upload returns, to be made into a media item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadToken(pub String);

/// An album's id.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(transparent)]
pub struct AlbumId(pub String);

/// A media item's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaId(pub String);

/// Why a call did not give what it was for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    /// No answer.
    #[error("the Photos API was not reached")]
    Unreached,
    /// An HTTP status that was not a success, and the seconds `Retry-After` named.
    #[error("the Photos API answered {status}")]
    Refused {
        /// The status.
        status: u16,
        /// The wait the server asked for.
        retry_after: Option<u32>,
    },
    /// An answer that was not the shape it should be.
    #[error("the Photos API answered something unreadable")]
    Unreadable,
    /// The call went through and the item failed (an unsupported format, a bad token).
    #[error("the item was refused: {message}")]
    ItemFailed {
        /// Google's status code for the item.
        code: i64,
        /// Its message.
        message: String,
    },
}

/// The media type Google Photos should treat `name`'s bytes as, by extension (`mime_guess`'s
/// table; its HEIC, HEIF and AVIF entries are what Photos takes). Only an image or a video type is
/// sent as such: any other extension, and none, is `application/octet-stream`.
pub fn mime_of(name: &str) -> &'static str {
    mime_guess::from_path(name)
        .first_raw()
        .filter(|mime| mime.starts_with("image/") || mime.starts_with("video/"))
        .unwrap_or("application/octet-stream")
}

#[derive(Deserialize)]
struct Album {
    id: String,
}

#[derive(Deserialize)]
struct Created {
    #[serde(default, rename = "newMediaItemResults")]
    results: Vec<ItemResult>,
}

#[derive(Deserialize)]
struct ItemResult {
    status: Option<Status>,
    #[serde(rename = "mediaItem")]
    item: Option<Item>,
}

#[derive(Deserialize)]
struct Status {
    code: Option<i64>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct Item {
    id: String,
}

fn refused(response: &Response<Vec<u8>>, now: UnixSeconds) -> ApiError {
    ApiError::Refused {
        status: response.status().as_u16(),
        retry_after: response.retry_after_seconds(now),
    }
}

/// The Library API at one origin.
#[derive(Debug)]
pub struct PhotosApi<H> {
    http: H,
    origin: String,
    clock: Clock,
}

impl<H: Http> PhotosApi<H> {
    /// The API served at the origin of `base` (`https://photoslibrary.googleapis.com/v1`),
    /// reached through `http`; a `Retry-After` date is counted from `clock` where the answer
    /// has no `Date`.
    pub fn new(http: H, base: &WebUrl, clock: Clock) -> Self {
        let text = base.as_str();
        let after = text.find("://").map_or(0, |at| at + 3);
        let end = text[after..]
            .find(['/', '?'])
            .map_or(text.len(), |at| after + at);
        Self {
            http,
            origin: text[..end].to_owned(),
            clock,
        }
    }

    fn request(&self, path: &str) -> Result<Builder, ApiError> {
        let url = WebUrl::parse(&format!("{}/v1/{path}", self.origin))
            .map_err(|_| ApiError::Unreadable)?;
        Ok(request(Method::POST, &url).header("Accept", "application/json"))
    }

    /// Sends the request `request` builds with `body`. A request that would not build is never
    /// sent, which is no answer, as a client that refuses it is.
    async fn send(&self, request: Builder, body: Vec<u8>) -> Result<Response<Vec<u8>>, ApiError> {
        let request = request.body(body).map_err(|_| ApiError::Unreached)?;
        let response = self
            .http
            .send(request)
            .await
            .map_err(|_| ApiError::Unreached)?;
        match response.status() {
            StatusCode::OK => Ok(response),
            _ => Err(refused(&response, self.clock.now())),
        }
    }

    /// Sends the bytes of a file (step one of an upload) and gets the token that makes them a
    /// media item.
    pub async fn upload(&self, name: &str, bytes: Vec<u8>) -> Result<UploadToken, ApiError> {
        let request = self
            .request("uploads")?
            .header("Content-Type", "application/octet-stream")
            .header("X-Goog-Upload-Content-Type", mime_of(name))
            .header("X-Goog-Upload-Protocol", "raw")
            .header("X-Goog-Upload-File-Name", name);
        let response = self.send(request, bytes).await?;
        let token = String::from_utf8(response.into_body()).map_err(|_| ApiError::Unreadable)?;
        let token = token.trim();
        match token.is_empty() {
            true => Err(ApiError::Unreadable),
            false => Ok(UploadToken(token.to_owned())),
        }
    }

    /// Makes an album titled `title` that the app owns.
    pub async fn create_album(&self, title: &str) -> Result<AlbumId, ApiError> {
        let body = serde_json::json!({"album": {"title": title}}).to_string();
        let request = self
            .request("albums")?
            .header("Content-Type", "application/json");
        let response = self.send(request, body.into_bytes()).await?;
        let album: Album =
            serde_json::from_slice(response.body()).map_err(|_| ApiError::Unreadable)?;
        Ok(AlbumId(album.id))
    }

    /// Makes the uploaded bytes behind `token` a media item named `name`, in `album`.
    pub async fn add_item(
        &self,
        album: Option<&AlbumId>,
        name: &str,
        token: &UploadToken,
    ) -> Result<MediaId, ApiError> {
        let mut body = serde_json::json!({
            "newMediaItems": [{"simpleMediaItem": {"fileName": name, "uploadToken": token.0}}],
        });
        if let Some(album) = album {
            body["albumId"] = serde_json::json!(album.0);
        }
        let request = self
            .request("mediaItems:batchCreate")?
            .header("Content-Type", "application/json");
        let response = self.send(request, body.to_string().into_bytes()).await?;
        let created: Created =
            serde_json::from_slice(response.body()).map_err(|_| ApiError::Unreadable)?;
        let result = created
            .results
            .into_iter()
            .next()
            .ok_or(ApiError::Unreadable)?;
        match (result.item, result.status) {
            (Some(item), status) if status.as_ref().and_then(|s| s.code).unwrap_or(0) == 0 => {
                Ok(MediaId(item.id))
            }
            (_, status) => Err(ApiError::ItemFailed {
                code: status.as_ref().and_then(|s| s.code).unwrap_or(-1),
                message: status.and_then(|s| s.message).unwrap_or_default(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_given_the_media_type_its_extension_says() {
        const CASES: &[(&str, &str)] = &[
            ("IMG_1.JPG", "image/jpeg"),
            ("a.b.png", "image/png"),
            ("clip.MOV", "video/quicktime"),
            ("x.heic", "image/heic"),
            ("x.HEIF", "image/heif"),
            ("x.avif", "image/avif"),
            ("a.webp", "image/webp"),
            ("a.mp4", "video/mp4"),
            ("a.jpe", "image/jpeg"),
            ("noext", "application/octet-stream"),
            ("notes.txt", "application/octet-stream"),
        ];
        for (name, want) in CASES {
            assert_eq!(mime_of(name), *want, "{name}");
        }
    }
}
