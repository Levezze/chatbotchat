//! HTTP client used by the `cbc` CLI and MCP wrapper. Thin typed wrapper over
//! `reqwest` against the chatbotchat daemon.

use chatbotchat_protocol::{
    ErrorEnvelope, JoinRoomRequest, JoinRoomResponse, LifecycleRequest, LifecycleResponse,
    OpenRoomRequest, OpenRoomResponse, RoomStatus, RoomSummary, RoomTranscript, SendMessageRequest,
    SendMessageResponse, SignalRequest, SignalResponse, WaitRequest, WaitResponse,
};

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

    /// Open a room. `hard_cap` / `soft_cap` are optional open-time overrides;
    /// `None` keeps the server defaults.
    pub async fn open_room(
        &self,
        subject: &str,
        hard_cap: Option<u32>,
        soft_cap: Option<u32>,
    ) -> Result<OpenRoomResponse, ClientError> {
        let resp = self
            .http
            .post(format!("{}/rooms", self.base_url))
            .json(&OpenRoomRequest {
                subject: subject.to_string(),
                hard_cap,
                soft_cap,
                // No CLI/MCP surface for continuation rooms yet; the server
                // accepts the link, the typed client will gain a param when the
                // open-from-prior flow lands.
                prev_room_id: None,
            })
            .send()
            .await?;
        decode(resp).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn join_room(
        &self,
        room_id: &str,
        repo: &str,
        model: &str,
        cwd: &str,
        instance: &str,
    ) -> Result<JoinRoomResponse, ClientError> {
        let resp = self
            .http
            .post(format!("{}/rooms/{room_id}/join", self.base_url))
            .json(&JoinRoomRequest {
                repo: repo.to_string(),
                model: model.to_string(),
                cwd: cwd.to_string(),
                instance: instance.to_string(),
            })
            .send()
            .await?;
        decode(resp).await
    }

    /// Post a `msg` to a room. `to == None` broadcasts to all participants.
    /// `from_human` flags a `--human` turn (folds the user's input in; resets the
    /// soft-cap counter).
    #[allow(clippy::too_many_arguments)]
    pub async fn send_message(
        &self,
        room_id: &str,
        repo: &str,
        model: &str,
        cwd: &str,
        instance: &str,
        to: Option<&str>,
        body: &str,
        from_human: bool,
    ) -> Result<SendMessageResponse, ClientError> {
        let resp = self
            .http
            .post(format!("{}/rooms/{room_id}/messages", self.base_url))
            .json(&SendMessageRequest {
                repo: repo.to_string(),
                model: model.to_string(),
                cwd: cwd.to_string(),
                instance: instance.to_string(),
                to: to.map(str::to_string),
                body: body.to_string(),
                from_human,
            })
            .send()
            .await?;
        decode(resp).await
    }

    /// Long-poll for the next message addressed to the caller (or broadcast).
    /// `max_wait_secs` optionally caps the server-side poll below its 10-minute
    /// cap (the MCP path uses this to return before a client tool-call timeout);
    /// `None` gets the full server cap. The per-request HTTP timeout sits just
    /// above the effective cap so the client never abandons the call early.
    #[allow(clippy::too_many_arguments)]
    pub async fn wait(
        &self,
        room_id: &str,
        repo: &str,
        model: &str,
        cwd: &str,
        instance: &str,
        max_wait_secs: Option<u32>,
    ) -> Result<WaitResponse, ClientError> {
        let http_timeout = match max_wait_secs {
            Some(secs) => std::time::Duration::from_secs(secs as u64 + 30),
            None => std::time::Duration::from_secs(660),
        };
        let resp = self
            .http
            .get(format!("{}/rooms/{room_id}/wait", self.base_url))
            .query(&WaitRequest {
                repo: repo.to_string(),
                model: model.to_string(),
                cwd: cwd.to_string(),
                instance: instance.to_string(),
                max_wait_secs,
            })
            .timeout(http_timeout)
            .send()
            .await?;
        decode(resp).await
    }

    /// Post a sentinel (out-of-band signal) to a room. `signal_type` is
    /// `waiting_user` or `fold`; `severity` + `question_text` are required for
    /// `waiting_user` and omitted for `fold`. Identity is the `(repo, model, cwd)`
    /// tuple, same as send; the caller must already be a participant.
    #[allow(clippy::too_many_arguments)]
    pub async fn signal(
        &self,
        room_id: &str,
        repo: &str,
        model: &str,
        cwd: &str,
        instance: &str,
        signal_type: &str,
        severity: Option<&str>,
        question_text: Option<&str>,
    ) -> Result<SignalResponse, ClientError> {
        let resp = self
            .http
            .post(format!("{}/rooms/{room_id}/signals", self.base_url))
            .json(&SignalRequest {
                repo: repo.to_string(),
                model: model.to_string(),
                cwd: cwd.to_string(),
                instance: instance.to_string(),
                signal_type: signal_type.to_string(),
                severity: severity.map(str::to_string),
                question_text: question_text.map(str::to_string),
                reason: None,
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

    /// List rooms (`cbc list`). `state` filters to one state; `all` includes
    /// archived. With neither, the server hides archived by default.
    pub async fn list_rooms(
        &self,
        state: Option<&str>,
        all: bool,
    ) -> Result<Vec<RoomSummary>, ClientError> {
        let mut req = self.http.get(format!("{}/rooms", self.base_url));
        if let Some(s) = state {
            req = req.query(&[("state", s)]);
        }
        if all {
            req = req.query(&[("all", "true")]);
        }
        let resp = req.send().await?;
        decode(resp).await
    }

    /// Fetch a room's full transcript (`cbc show`): metadata, caps and counters,
    /// participants, and every message oldest-first.
    pub async fn transcript(&self, room_id: &str) -> Result<RoomTranscript, ClientError> {
        let resp = self
            .http
            .get(format!("{}/rooms/{room_id}/transcript", self.base_url))
            .send()
            .await?;
        decode(resp).await
    }

    /// Explicitly close a room. Identity is the `(repo, model, cwd)` tuple; the
    /// caller must be a participant. Returns the room's new state.
    pub async fn close(
        &self,
        room_id: &str,
        repo: &str,
        model: &str,
        cwd: &str,
        instance: &str,
    ) -> Result<LifecycleResponse, ClientError> {
        self.lifecycle(room_id, "close", repo, model, cwd, instance, None)
            .await
    }

    /// Pause a room, optionally recording `reason` in the audit log. Identity is
    /// the `(repo, model, cwd)` tuple; the caller must be a participant.
    pub async fn pause(
        &self,
        room_id: &str,
        repo: &str,
        model: &str,
        cwd: &str,
        instance: &str,
        reason: Option<&str>,
    ) -> Result<LifecycleResponse, ClientError> {
        self.lifecycle(room_id, "pause", repo, model, cwd, instance, reason)
            .await
    }

    /// Wake a paused (or idle) room back to active. Identity is the caller's
    /// `instance`; the caller must be a participant.
    pub async fn wake(
        &self,
        room_id: &str,
        repo: &str,
        model: &str,
        cwd: &str,
        instance: &str,
    ) -> Result<LifecycleResponse, ClientError> {
        self.lifecycle(room_id, "wake", repo, model, cwd, instance, None)
            .await
    }

    /// Shared POST for the close/pause/wake lifecycle endpoints.
    #[allow(clippy::too_many_arguments)]
    async fn lifecycle(
        &self,
        room_id: &str,
        op: &str,
        repo: &str,
        model: &str,
        cwd: &str,
        instance: &str,
        reason: Option<&str>,
    ) -> Result<LifecycleResponse, ClientError> {
        let resp = self
            .http
            .post(format!("{}/rooms/{room_id}/{op}", self.base_url))
            .json(&LifecycleRequest {
                repo: repo.to_string(),
                model: model.to_string(),
                cwd: cwd.to_string(),
                instance: instance.to_string(),
                reason: reason.map(str::to_string),
            })
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
