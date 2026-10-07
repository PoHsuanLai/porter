//! The Photos Library API calls an upload makes: bytes to an upload token, an album, and the
//! token made into a media item. Pure over any [`Http`]; in syncd the `Http` is a relay that adds
//! the bearer.

use porter_core::WebUrl;
use porter_http::{Http, HttpRequest, HttpResponse, Method};
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

/// The media type Google Photos should treat `name`'s bytes as, by extension.
pub fn mime_of(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map_or("", |(_, ext)| ext)
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "3gp" => "video/3gpp",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
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

fn refused(response: &HttpResponse) -> ApiError {
    ApiError::Refused {
        status: response.status.0,
        retry_after: response
            .header("retry-after")
            .and_then(|v| v.trim().parse().ok()),
    }
}

/// The Library API at one origin.
#[derive(Debug)]
pub struct PhotosApi<H> {
    http: H,
    origin: String,
}

impl<H: Http> PhotosApi<H> {
    /// The API served at the origin of `base` (`https://photoslibrary.googleapis.com/v1`),
    /// reached through `http`.
    pub fn new(http: H, base: &WebUrl) -> Self {
        let text = base.as_str();
        let after = text.find("://").map_or(0, |at| at + 3);
        let end = text[after..]
            .find(['/', '?'])
            .map_or(text.len(), |at| after + at);
        Self {
            http,
            origin: text[..end].to_owned(),
        }
    }

    fn request(&self, path: &str) -> Result<HttpRequest, ApiError> {
        let url = WebUrl::parse(&format!("{}/v1/{path}", self.origin))
            .map_err(|_| ApiError::Unreadable)?;
        Ok(HttpRequest::new(Method::Post, url))
    }

    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ApiError> {
        let response = self
            .http
            .send(request.with_header("Accept", "application/json"))
            .await
            .map_err(|_| ApiError::Unreached)?;
        match response.status.0 {
            200 => Ok(response),
            _ => Err(refused(&response)),
        }
    }

    /// Sends the bytes of a file (step one of an upload) and gets the token that makes them a
    /// media item.
    pub async fn upload(&self, name: &str, bytes: Vec<u8>) -> Result<UploadToken, ApiError> {
        let request = self
            .request("uploads")?
            .with_header("Content-Type", "application/octet-stream")
            .with_header("X-Goog-Upload-Content-Type", mime_of(name))
            .with_header("X-Goog-Upload-Protocol", "raw")
            .with_header("X-Goog-Upload-File-Name", name)
            .with_body(bytes);
        let response = self.send(request).await?;
        let token = String::from_utf8(response.body).map_err(|_| ApiError::Unreadable)?;
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
            .with_header("Content-Type", "application/json")
            .with_body(body);
        let response = self.send(request).await?;
        let album: Album =
            serde_json::from_slice(&response.body).map_err(|_| ApiError::Unreadable)?;
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
            .with_header("Content-Type", "application/json")
            .with_body(body.to_string());
        let response = self.send(request).await?;
        let created: Created =
            serde_json::from_slice(&response.body).map_err(|_| ApiError::Unreadable)?;
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
            ("noext", "application/octet-stream"),
            ("notes.txt", "application/octet-stream"),
        ];
        for (name, want) in CASES {
            assert_eq!(mime_of(name), *want, "{name}");
        }
    }
}
