//! HTTP client used by the `cbc` CLI and MCP wrapper. Thin typed wrapper over
//! `reqwest` against the chatbotchat daemon.

use chatbotchat_protocol::{ErrorEnvelope, OpenRoomRequest, OpenRoomResponse, RoomStatus};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("server error ({status}): {message}")]
    Server { status: u16, message: String },
}

#[derive(Clone)]
pub struct HttpClient {
    base_url: String,
    http: reqwest::Client,
}

impl HttpClient {
    /// Create a client targeting `base_url`, e.g. `http://127.0.0.1:8484`.
    /// A trailing slash is trimmed so path joins stay clean.
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        HttpClient {
            base_url,
            http: reqwest::Client::new(),
        }
    }

    pub async fn open_room(&self, subject: &str) -> Result<OpenRoomResponse, ClientError> {
        let resp = self
            .http
            .post(format!("{}/rooms", self.base_url))
            .json(&OpenRoomRequest {
                subject: subject.to_string(),
            })
            .send()
            .await?;
        decode(resp).await
    }

    pub async fn status(&self, room_id: &str) -> Result<RoomStatus, ClientError> {
        let resp = self
            .http
            .get(format!("{}/rooms/{room_id}", self.base_url))
            .send()
            .await?;
        decode(resp).await
    }
}

/// Decode a JSON body on 2xx, or surface the server's `ErrorEnvelope` message.
async fn decode<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T, ClientError> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp.json::<T>().await?)
    } else {
        let code = status.as_u16();
        let message = match resp.json::<ErrorEnvelope>().await {
            Ok(env) => env.error,
            Err(_) => status
                .canonical_reason()
                .unwrap_or("unknown error")
                .to_string(),
        };
        Err(ClientError::Server {
            status: code,
            message,
        })
    }
}
