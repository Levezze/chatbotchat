//! One private local endpoint for every bound member and room. No native identity inference.
//! Credentials/references in this wire protocol are adapter-only, never model-facing output.
use crate::{
    bindings::SessionBinding,
    dashboard::session::Sessions,
    enrollment::{Connection, ConnectionKind, DirectoryEntry},
    facts::{WakeKind, WakeOutcome, WakeReport},
    inbox::{Batch, InboxChange, Item, MAX_BATCH_HARD_BYTES, MAX_ITEMS},
    owner::{OwnerControl, ProvisionSpec},
    settings::{DispatchPolicy, RuntimeSettings, RuntimeSettingsService, SettingsSnapshot},
    store::Store,
    Sandbox,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    future::Future,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{watch, Semaphore},
    task::JoinSet,
};

mod requests;
mod work;

const MAX_REQUEST_BYTES: usize = 128 * 1024;
const MAX_REPLY_BYTES: usize = MAX_BATCH_HARD_BYTES + 8192;
const MAX_CLIENTS: usize = 32;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct LocalConnector {
    listener: UnixListener,
    socket: SocketGuard,
    store: Store,
    settings: Arc<Mutex<RuntimeSettingsService>>,
    owner: Arc<OwnerControl>,
    dashboard: Option<(tokio::net::TcpListener, Arc<Sessions>)>,
}
impl LocalConnector {
    /// Bind only a new socket inside the validated private candidate root. Never replace paths.
    pub fn bind(sandbox: &Sandbox, store: Store) -> Result<Self> {
        let sandbox = Sandbox::inspect(sandbox.root(), &sandbox.manifest.protected_databases)?;
        let path = sandbox.root().join("connector.sock");
        ensure!(
            path.as_os_str().len() < 104,
            "candidate socket path is too long"
        );
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => anyhow::bail!("candidate socket path already exists or is inaccessible"),
        }
        let listener = UnixListener::bind(&path)?;
        let meta = fs::symlink_metadata(&path)?;
        let socket = SocketGuard {
            path,
            device: meta.dev(),
            inode: meta.ino(),
        };
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))?;
        ensure!(
            meta.file_type().is_socket() && meta.uid() == crate::effective_uid(),
            "unsafe candidate socket"
        );
        let settings = Arc::new(Mutex::new(RuntimeSettingsService::open(&sandbox)?));
        let owner = Arc::new(OwnerControl::open(&sandbox)?);
        Ok(Self {
            listener,
            socket,
            store,
            settings,
            owner,
            dashboard: None,
        })
    }
    /// Also serve the dashboard on `listener`, which must be on 127.0.0.1. Its sessions live in
    /// this process only; the owner route mints the login codes.
    pub fn with_dashboard(mut self, listener: tokio::net::TcpListener) -> Result<Self> {
        crate::dashboard::check_listener(&listener)?;
        self.dashboard = Some((listener, Sessions::new()));
        Ok(self)
    }
    pub fn path(&self) -> &Path {
        &self.socket.path
    }
    /// Idle means awaiting the socket; there is no timer, room poll or model invocation.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        let Self {
            listener,
            socket,
            store,
            settings,
            owner,
            dashboard,
        } = self;
        let store = Arc::new(store);
        let (sessions, dashboard) = match dashboard {
            Some((listener, sessions)) => {
                let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
                let serve =
                    crate::dashboard::serve(listener, store.clone(), sessions.clone(), async {
                        let _ = stopped.await;
                    });
                // Say so at once if the dashboard stops early; the error is also returned at exit.
                let task = tokio::spawn(async move {
                    let outcome = serve.await;
                    if let Err(error) = &outcome {
                        eprintln!("dashboard stopped: {error:#}");
                        tracing::error!(event = "dashboard_stopped");
                    }
                    outcome
                });
                (Some(sessions), Some((stop, task)))
            }
            None => (None, None),
        };
        let slots = Arc::new(Semaphore::new(MAX_CLIENTS));
        let (notices, _) = watch::channel(0_u64);
        let mut workers = JoinSet::new();
        let mut accept_backoff = Duration::from_millis(25);
        tokio::pin!(shutdown);
        let outcome = loop {
            tokio::select! {
                _ = &mut shutdown => break Ok(()),
                _ = workers.join_next(), if !workers.is_empty() => {},
                accepted = listener.accept() => {
                    let (mut stream,_) = match accepted {
                        Ok(v)=>{accept_backoff=Duration::from_millis(25);v},
                        Err(e) if transient_accept(&e) => {
                            tracing::warn!(event = "accept_unavailable", kind = ?e.kind());
                            tokio::select! {
                                _ = &mut shutdown => break Ok(()),
                                _ = tokio::time::sleep(accept_backoff) => {},
                            }
                            accept_backoff = (accept_backoff * 2).min(Duration::from_secs(1));
                            continue;
                        },
                        Err(e)=>break Err(e.into()),
                    };
                    let permit = match slots.clone().try_acquire_owned() {
                        Ok(p)=>p,
                        Err(_)=>{
                            let _ = tokio::time::timeout(Duration::from_secs(1),stream.write_all(b"{\"type\":\"error\",\"message\":\"candidate busy; retry\"}\n")).await;
                            continue;
                        }
                    };
                    let store = store.clone();
                    let notices = notices.clone();
                    let settings = settings.clone();
                    let owner = owner.clone();
                    let sessions = sessions.clone();
                    workers.spawn(async move {
                        let _permit = permit;
                        let _ = serve_one(stream,&store,&settings,&owner,sessions.as_deref(),&notices).await;
                    });
                }
            }
        };
        drop(listener);
        workers.abort_all();
        while workers.join_next().await.is_some() {}
        let outcome = match dashboard {
            Some((stop, mut task)) => {
                let _ = stop.send(());
                match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
                    Ok(Ok(Ok(()))) => outcome,
                    Ok(Ok(Err(error))) => outcome.and(Err(error)),
                    Ok(Err(error)) => outcome.and(Err(error.into())),
                    Err(_) => {
                        task.abort();
                        let _ = task.await;
                        outcome.and(Err(anyhow::anyhow!("dashboard did not stop in time")))
                    }
                }
            }
            None => outcome,
        };
        let store = Arc::try_unwrap(store)
            .map_err(|_| anyhow::anyhow!("candidate clients still hold the store"))?;
        store.close().await;
        drop(socket);
        outcome
    }
}

pub(crate) fn transient_accept(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::ConnectionAborted
    ) || error.raw_os_error().is_some_and(|code| {
        [libc::EMFILE, libc::ENFILE, libc::ENOBUFS, libc::ENOMEM].contains(&code)
    })
}
struct SocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Ok(meta) = fs::symlink_metadata(&self.path) {
            if meta.file_type().is_socket() && (meta.dev(), meta.ino()) == (self.device, self.inode)
            {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}
// Deliberately no Debug/Display. SessionBinding itself remains non-serializable.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Auth {
    context: String,
    member: String,
    device: String,
    session: String,
    epoch: i64,
    token: String,
}
impl From<&SessionBinding> for Auth {
    fn from(b: &SessionBinding) -> Self {
        Self {
            context: b.context_id.clone(),
            member: b.member_id.clone(),
            device: b.device_id.clone(),
            session: b.native_session_id.clone(),
            epoch: b.epoch,
            token: b.expose_token_for_adapter().into(),
        }
    }
}
impl Auth {
    fn binding(&self) -> Result<SessionBinding> {
        SessionBinding::from_token(
            &self.context,
            &self.member,
            &self.device,
            &self.session,
            self.epoch,
            &self.token,
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    v: u8,
    auth: Auth,
    request: Operation,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Inbound {
    Member(Request),
    Owner(OwnerRequest),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerRequest {
    v: u8,
    owner: String,
    request: OwnerOperation,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum OwnerOperation {
    Provision {
        spec: ProvisionSpec,
    },
    RevokeBinding {
        context: String,
        member: String,
        expected_epoch: i64,
    },
    RetireMember {
        context: String,
        member: String,
        reason: String,
    },
    Settings,
    UpdateSettings {
        expected_revision: String,
        settings: RuntimeSettings,
    },
    Work {
        work: work::OwnerWorkOperation,
    },
    Request {
        request: requests::OwnerRequestOperation,
    },
    DashboardLogin,
    Snapshot,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Check,
    BindingStatus,
    Settings,
    Discover {
        limit: usize,
    },
    Connect {
        target: String,
        kind: ConnectionKind,
        scope_ref: String,
        key: String,
        explanation: String,
    },
    Watch {
        others: Vec<Auth>,
        after: i64,
    },
    WatchMembers {
        others: Vec<Auth>,
        after: i64,
    },
    Send {
        room: String,
        key: String,
        body: String,
    },
    Receipt {
        reference: String,
    },
    Close {
        room: String,
        reason: String,
    },
    Work {
        work: work::WorkOperation,
    },
    Request {
        request: requests::RequestOperation,
    },
    /// The device driver's report of one wake attempt for this binding (#113 B1). Transport:
    /// it never counts as the member's own activity.
    Wake {
        through: i64,
        kind: WakeKind,
        outcome: WakeOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<String>,
        recovery: bool,
        attempt_wall_us: i64,
    },
}

impl Operation {
    /// The name recorded as the member's first activity after a receipt. None for transport:
    /// reading, receipting, status, settings, watching and wake reports are done by the host
    /// adapters on the member's behalf, so they say nothing about the member itself.
    /// The operation's wire name, for the debug log of a rejected request.
    fn name(&self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::BindingStatus => "binding_status",
            Self::Settings => "settings",
            Self::Discover { .. } => "discover",
            Self::Connect { .. } => "connect",
            Self::Watch { .. } => "watch",
            Self::WatchMembers { .. } => "watch_members",
            Self::Send { .. } => "send",
            Self::Receipt { .. } => "receipt",
            Self::Close { .. } => "close",
            Self::Work { .. } => "work",
            Self::Request { .. } => "request",
            Self::Wake { .. } => "wake",
        }
    }

    fn activity(&self) -> Option<&'static str> {
        match self {
            Self::Discover { .. } => Some("discover"),
            Self::Connect { .. } => Some("connect"),
            Self::Send { .. } => Some("send"),
            Self::Close { .. } => Some("close"),
            Self::Work { .. } => Some("work"),
            Self::Request { .. } => Some("request"),
            Self::Check
            | Self::BindingStatus
            | Self::Settings
            | Self::Watch { .. }
            | Self::WatchMembers { .. }
            | Self::Receipt { .. }
            | Self::Wake { .. } => None,
        }
    }
}

/// Content-free snapshot for the device driver, with the recovery flag the core derives from
/// the member's live settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingStatus {
    pub last_receipted_sequence: i64,
    pub first_unread_sequence: Option<i64>,
    pub native_target: Option<crate::native_target::NativeTarget>,
    pub recovery_due: bool,
    /// The live log level, so the driver follows the core's settings (B2).
    pub log_level: crate::sampler::Mode,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Empty,
    BindingStatus {
        last_receipted_sequence: i64,
        first_unread_sequence: Option<i64>,
        #[serde(deserialize_with = "crate::native_target::required_target")]
        native_target: Option<crate::native_target::NativeTarget>,
        recovery_due: bool,
        #[serde(default)]
        log_level: crate::sampler::Mode,
    },
    Woken {
        covered: u64,
    },
    OwnerResult {
        value: serde_json::Value,
    },
    Settings {
        #[serde(flatten)]
        settings: BoundSettings,
    },
    Directory {
        members: Vec<DirectoryEntry>,
    },
    Connected {
        connection: Connection,
    },
    Changed {
        sequence: i64,
    },
    MembersChanged {
        sequence: i64,
        members: Vec<InboxChange>,
    },
    Batch {
        items: Vec<Item>,
        reference: String,
    },
    Sent {
        sequence: i64,
    },
    Receipted,
    Closed {
        sequence: i64,
    },
    Work {
        result: work::WorkReply,
    },
    Request {
        result: requests::RequestReply,
    },
    Error {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
}
async fn frame(stream: &mut UnixStream, max: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    BufReader::new(stream)
        .take((max + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .await?;
    ensure!(
        bytes.len() <= max && bytes.last() == Some(&b'\n'),
        "invalid or oversized candidate frame"
    );
    Ok(bytes)
}
async fn serve_one(
    mut stream: UnixStream,
    store: &Store,
    settings: &Mutex<RuntimeSettingsService>,
    owner: &OwnerControl,
    sessions: Option<&Sessions>,
    notices: &watch::Sender<u64>,
) -> Result<()> {
    ensure!(
        stream.peer_cred()?.uid() == crate::effective_uid(),
        "candidate peer rejected"
    );
    let reply =
        match tokio::time::timeout(REQUEST_TIMEOUT, frame(&mut stream, MAX_REQUEST_BYTES)).await {
            Ok(Ok(bytes)) => match serde_json::from_slice::<Inbound>(&bytes) {
                Ok(Inbound::Member(request)) => {
                    match route(store, settings, request, notices, &mut stream).await {
                        Ok(reply) => reply,
                        Err(_) => Reply::Error {
                            message: "candidate operation rejected".into(),
                            code: None,
                        },
                    }
                }
                Ok(Inbound::Owner(request)) => match tokio::time::timeout(
                    REQUEST_TIMEOUT,
                    owner_route(store, settings, owner, sessions, request),
                )
                .await
                {
                    Ok(Ok(value)) => {
                        notices.send_modify(|revision| *revision = revision.wrapping_add(1));
                        Reply::OwnerResult { value }
                    }
                    _ => Reply::Error {
                        message: "candidate owner operation rejected or uncertain".into(),
                        code: None,
                    },
                },
                Err(_) => Reply::Error {
                    message: "invalid candidate request".into(),
                    code: None,
                },
            },
            _ => Reply::Error {
                message: "invalid candidate frame".into(),
                code: None,
            },
        };
    let mut bytes = serde_json::to_vec(&reply)?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= MAX_REPLY_BYTES,
        "candidate reply exceeds bound"
    );
    write_reply(&mut stream, &bytes, REQUEST_TIMEOUT).await?;
    Ok(())
}

async fn write_reply(stream: &mut UnixStream, bytes: &[u8], deadline: Duration) -> Result<()> {
    tokio::time::timeout(deadline, async {
        stream.write_all(bytes).await?;
        stream.shutdown().await
    })
    .await
    .context("candidate reply timed out")??;
    Ok(())
}

async fn owner_route(
    store: &Store,
    settings: &Mutex<RuntimeSettingsService>,
    owner: &OwnerControl,
    sessions: Option<&Sessions>,
    request: OwnerRequest,
) -> Result<serde_json::Value> {
    ensure!(
        request.v == 1 && owner.authenticate(&request.owner),
        "candidate owner rejected"
    );
    match request.request {
        OwnerOperation::Provision { spec } => owner.provision(store, spec).await,
        OwnerOperation::RevokeBinding {
            context,
            member,
            expected_epoch,
        } => {
            let epoch = store
                .revoke_binding(&context, &member, expected_epoch)
                .await?;
            Ok(serde_json::json!({"epoch":epoch}))
        }
        OwnerOperation::RetireMember {
            context,
            member,
            reason,
        } => {
            store.retire_member(&context, &member, &reason).await?;
            Ok(serde_json::json!({"retired":true}))
        }
        OwnerOperation::Settings => {
            let snapshot = settings
                .lock()
                .map_err(|_| anyhow::anyhow!("settings unavailable"))?
                .refresh();
            Ok(serde_json::to_value(snapshot)?)
        }
        OwnerOperation::UpdateSettings {
            expected_revision,
            settings: update,
        } => {
            let snapshot = settings
                .lock()
                .map_err(|_| anyhow::anyhow!("settings unavailable"))?
                .update(&expected_revision, update)?;
            Ok(serde_json::to_value(snapshot)?)
        }
        OwnerOperation::Work { work } => work::owner_dispatch(store, work).await,
        OwnerOperation::Request { request } => requests::owner_dispatch(store, request).await,
        OwnerOperation::Snapshot => Ok(serde_json::to_value(
            crate::evidence::take_snapshot(store, owner.root()).await?,
        )?),
        OwnerOperation::DashboardLogin => Ok(serde_json::json!({
            "code": sessions.context("no dashboard is running")?.mint_code(),
        })),
    }
}

async fn route(
    store: &Store,
    settings: &Mutex<RuntimeSettingsService>,
    request: Request,
    notices: &watch::Sender<u64>,
    stream: &mut UnixStream,
) -> Result<Reply> {
    ensure!(request.v == 1, "unsupported candidate protocol");
    // A request is logged when debug holds at its start and at its end, so normal mode pays
    // nothing per request and a request that switches the level is never logged. A rejected
    // request is logged by its operation; the error's text is never logged.
    let asker = tracing::enabled!(tracing::Level::DEBUG).then(|| {
        (
            request.auth.context.clone(),
            request.auth.member.clone(),
            request.request.name(),
        )
    });
    let answered = answer(store, settings, request, notices, stream).await;
    if let Some((context, member, operation)) = asker {
        match &answered {
            Ok(reply) => crate::log::reply(&context, &member, &serde_json::to_value(reply)?),
            Err(_) => crate::log::rejected(&context, &member, operation),
        }
    }
    answered
}

async fn answer(
    store: &Store,
    settings: &Mutex<RuntimeSettingsService>,
    request: Request,
    notices: &watch::Sender<u64>,
    stream: &mut UnixStream,
) -> Result<Reply> {
    let Request { v, auth, request } = request;
    if matches!(
        request,
        Operation::Watch { .. } | Operation::WatchMembers { .. }
    ) {
        let (others, after, targeted) = match request {
            Operation::Watch { others, after } => (others, after, false),
            Operation::WatchMembers { others, after } => (others, after, true),
            _ => unreachable!(),
        };
        ensure!(after >= 0 && others.len() < 64, "invalid subscription");
        // Subscribe before reading durable state, so commits between the query and wait cannot vanish.
        let mut rx = notices.subscribe();
        let mut bindings = vec![auth.binding()?];
        for other in others {
            bindings.push(other.binding()?);
        }
        loop {
            let members = store.inbox_changes(&bindings, after).await?;
            if let Some(sequence) = members.iter().map(|change| change.sequence).max() {
                return Ok(if targeted {
                    Reply::MembersChanged { sequence, members }
                } else {
                    Reply::Changed { sequence }
                });
            }
            let mut unexpected = [0_u8; 1];
            tokio::select! {
                changed = rx.changed() => {changed.context("candidate feed closed")?;},
                closed = stream.read(&mut unexpected) => {
                    closed?;
                    anyhow::bail!("candidate feed client closed or sent extra input");
                }
            }
        }
    }
    let reply = tokio::time::timeout(
        REQUEST_TIMEOUT,
        dispatch(store, settings, Request { v, auth, request }),
    )
    .await
    .context("candidate operation timed out")??;
    if matches!(
        &reply,
        Reply::Sent { .. } | Reply::Closed { .. } | Reply::Connected { .. }
    ) || matches!(&reply, Reply::Work { result } if result.committed())
        || matches!(&reply, Reply::Request { result } if result.committed())
    {
        notices.send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    Ok(reply)
}
async fn dispatch(
    store: &Store,
    settings: &Mutex<RuntimeSettingsService>,
    request: Request,
) -> Result<Reply> {
    let started = store.request_start();
    ensure!(request.v == 1, "unsupported candidate protocol");
    let binding = request.auth.binding()?;
    if let Some(operation) = request.request.activity() {
        store
            .record_activity_since(&binding, operation, &started)
            .await?;
    }
    Ok(match request.request {
        Operation::BindingStatus => match store.binding_activity(&binding).await? {
            Some(activity) => {
                // Fail-soft: the native waiter, the Codex latch and startup verification all read
                // this status, so a recovery computation error must never fail it. A false flag
                // only skips one recovery pass.
                let recovery_due = async {
                    let member = store.current_member(&binding).await?;
                    let policy = settings
                        .lock()
                        .map_err(|_| anyhow::anyhow!("settings unavailable"))?
                        .refresh()
                        .effective(&member.context_id, member.role.as_str(), &member.member_id)
                        .policy;
                    let interval = i64::try_from(policy.recovery_after_ms).unwrap_or(i64::MAX);
                    store
                        .recovery_due(&binding, interval, policy.recovery_attempts)
                        .await
                }
                .await
                .unwrap_or(false);
                let log_level = settings
                    .lock()
                    .map(|service| service.snapshot().settings.log_level)
                    .unwrap_or_default();
                Reply::BindingStatus {
                    last_receipted_sequence: activity.last_receipted_sequence,
                    first_unread_sequence: activity.first_unread_sequence,
                    native_target: activity.native_target,
                    recovery_due,
                    log_level,
                }
            }
            None => Reply::Error {
                code: Some("invalid_binding".into()),
                message: "invalid or superseded binding".into(),
            },
        },
        Operation::Settings => {
            let member = store.current_member(&binding).await?;
            let snapshot = settings
                .lock()
                .map_err(|_| anyhow::anyhow!("settings unavailable"))?
                .refresh();
            let effective =
                snapshot.effective(&member.context_id, member.role.as_str(), &member.member_id);
            Reply::Settings {
                settings: BoundSettings {
                    revision: snapshot.revision,
                    policy: effective.policy,
                    sources: effective.sources,
                    valid: snapshot.last_error.is_none(),
                },
            }
        }
        Operation::Discover { limit } => Reply::Directory {
            members: store.discover_connections(&binding, limit).await?,
        },
        Operation::Connect {
            target,
            kind,
            scope_ref,
            key,
            explanation,
        } => Reply::Connected {
            connection: store
                .connect_members(&binding, &target, kind, &scope_ref, &key, &explanation)
                .await?,
        },
        Operation::Watch { .. } | Operation::WatchMembers { .. } => {
            anyhow::bail!("watch requires the feed route")
        }
        Operation::Check => match store.check(&binding, MAX_ITEMS).await? {
            Some(batch) => Reply::Batch {
                items: batch.items,
                reference: batch.reference,
            },
            None => Reply::Empty,
        },
        Operation::Send { room, key, body } => {
            // Cadence belongs to the report parent or the shared peer context, never
            // whichever participant happens to send the threshold-crossing message.
            let owner = store.room_notice_policy_owner(&binding, &room).await?;
            let snapshot = settings
                .lock()
                .map_err(|_| anyhow::anyhow!("settings unavailable"))?
                .refresh();
            let (member, role) = owner
                .as_ref()
                .map(|(member, role)| (member.as_str(), role.as_str()))
                .unwrap_or(("", ""));
            let interval = snapshot
                .effective(&binding.context_id, role, member)
                .policy
                .room_notice_every_messages;
            Reply::Sent {
                sequence: store
                    .send_with_notice_interval(&binding, &room, &key, &body, interval)
                    .await?,
            }
        }
        Operation::Receipt { reference } => {
            store.receipt(&binding, &reference).await?;
            Reply::Receipted
        }
        Operation::Close { room, reason } => Reply::Closed {
            sequence: store.close_room(&binding, &room, &reason).await?,
        },
        Operation::Work { work } => Reply::Work {
            result: work::dispatch(store, &binding, work).await?,
        },
        Operation::Request { request } => Reply::Request {
            result: requests::dispatch(store, &binding, request).await?,
        },
        Operation::Wake {
            through,
            kind,
            outcome,
            failure,
            recovery,
            attempt_wall_us,
        } => Reply::Woken {
            covered: store
                .record_wake(
                    &binding,
                    &WakeReport {
                        through,
                        kind,
                        outcome,
                        failure,
                        recovery,
                        attempt_wall_us,
                    },
                )
                .await?,
        },
    })
}
/// Trusted host adapter client. The model supplies neither a member ID nor a receipt reference.
pub struct LocalClient<'a> {
    path: &'a Path,
    binding: &'a SessionBinding,
}
/// Local owner administration. Credentials and provisioning results are not model tool data.
pub struct LocalOwnerClient<'a> {
    path: &'a Path,
    token: &'a str,
}
impl<'a> LocalOwnerClient<'a> {
    pub fn new(path: &'a Path, token: &'a str) -> Self {
        Self { path, token }
    }
    async fn call(&self, operation: OwnerOperation) -> Result<serde_json::Value> {
        let mut stream =
            tokio::time::timeout(REQUEST_TIMEOUT, UnixStream::connect(self.path)).await??;
        ensure!(
            stream.peer_cred()?.uid() == crate::effective_uid(),
            "candidate owner server rejected"
        );
        let mut bytes = serde_json::to_vec(&OwnerRequest {
            v: 1,
            owner: self.token.into(),
            request: operation,
        })?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() <= MAX_REQUEST_BYTES,
            "candidate owner request exceeds bound"
        );
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            stream.write_all(&bytes).await?;
            match serde_json::from_slice::<Reply>(&frame(&mut stream, MAX_REPLY_BYTES).await?)? {
                Reply::OwnerResult { value } => Ok(value),
                Reply::Error { message, .. } => anyhow::bail!(message),
                _ => anyhow::bail!("unexpected candidate owner reply"),
            }
        })
        .await
        .context("candidate owner operation timed out")?
    }
    /// A single-use code for `/login`, valid for 30 seconds. It belongs in the browser the
    /// user's own command opens, never in output a model reads.
    pub async fn dashboard_login_code(&self) -> Result<String> {
        let value = self.call(OwnerOperation::DashboardLogin).await?;
        Ok(value["code"]
            .as_str()
            .context("invalid candidate login code")?
            .to_owned())
    }
    /// C1: the running core freezes its database into a new private snapshot file.
    pub async fn snapshot(&self) -> Result<crate::evidence::Snapshot> {
        Ok(serde_json::from_value(
            self.call(OwnerOperation::Snapshot).await?,
        )?)
    }
    pub async fn provision(&self, spec: ProvisionSpec) -> Result<serde_json::Value> {
        self.call(OwnerOperation::Provision { spec }).await
    }
    pub async fn revoke_binding(
        &self,
        context: &str,
        member: &str,
        expected_epoch: i64,
    ) -> Result<i64> {
        self.call(OwnerOperation::RevokeBinding {
            context: context.into(),
            member: member.into(),
            expected_epoch,
        })
        .await?["epoch"]
            .as_i64()
            .context("invalid candidate revocation result")
    }
    /// T-25: end an identity with its session. It can never be bound again.
    pub async fn retire_member(&self, context: &str, member: &str, reason: &str) -> Result<()> {
        ensure!(
            self.call(OwnerOperation::RetireMember {
                context: context.into(),
                member: member.into(),
                reason: reason.into(),
            })
            .await?["retired"]
                == serde_json::Value::Bool(true),
            "invalid candidate retirement result"
        );
        Ok(())
    }
    pub async fn settings(&self) -> Result<SettingsSnapshot> {
        Ok(serde_json::from_value(
            self.call(OwnerOperation::Settings).await?,
        )?)
    }
    pub async fn update_settings(
        &self,
        revision: &str,
        settings: RuntimeSettings,
    ) -> Result<SettingsSnapshot> {
        Ok(serde_json::from_value(
            self.call(OwnerOperation::UpdateSettings {
                expected_revision: revision.into(),
                settings,
            })
            .await?,
        )?)
    }
}
/// Only the authenticated member's effective policy; other contexts' overrides stay private.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundSettings {
    pub revision: String,
    pub policy: DispatchPolicy,
    pub sources: Vec<String>,
    pub valid: bool,
}
impl<'a> LocalClient<'a> {
    pub async fn binding_status(&self) -> Result<BindingStatus> {
        match self.call(Operation::BindingStatus).await? {
            Reply::BindingStatus {
                last_receipted_sequence,
                first_unread_sequence,
                native_target,
                recovery_due,
                log_level,
            } => Ok(BindingStatus {
                last_receipted_sequence,
                first_unread_sequence,
                native_target,
                recovery_due,
                log_level,
            }),
            _ => anyhow::bail!("unexpected candidate response"),
        }
    }
    /// Returns how many unread items the attempt covered.
    pub async fn report_wake(&self, report: &WakeReport) -> Result<u64> {
        let report = report.clone();
        match self
            .call(Operation::Wake {
                through: report.through,
                kind: report.kind,
                outcome: report.outcome,
                failure: report.failure,
                recovery: report.recovery,
                attempt_wall_us: report.attempt_wall_us,
            })
            .await?
        {
            Reply::Woken { covered } => Ok(covered),
            _ => anyhow::bail!("unexpected candidate response"),
        }
    }
    pub async fn settings(&self) -> Result<BoundSettings> {
        match self.call(Operation::Settings).await? {
            Reply::Settings { settings } => Ok(settings),
            _ => anyhow::bail!("unexpected candidate response"),
        }
    }
    pub async fn discover(&self, limit: usize) -> Result<Vec<DirectoryEntry>> {
        match self.call(Operation::Discover { limit }).await? {
            Reply::Directory { members } => Ok(members),
            _ => anyhow::bail!("unexpected candidate response"),
        }
    }
    pub async fn connect(
        &self,
        target: &str,
        kind: ConnectionKind,
        scope_ref: &str,
        key: &str,
        explanation: &str,
    ) -> Result<Connection> {
        match self
            .call(Operation::Connect {
                target: target.into(),
                kind,
                scope_ref: scope_ref.into(),
                key: key.into(),
                explanation: explanation.into(),
            })
            .await?
        {
            Reply::Connected { connection } => Ok(connection),
            _ => anyhow::bail!("unexpected candidate response"),
        }
    }
    /// One parked connection returns only changed authorized members, without freezing batches.
    pub async fn watch_members(
        &self,
        others: &[&SessionBinding],
        after: i64,
    ) -> Result<Vec<InboxChange>> {
        match self
            .exchange(Operation::WatchMembers {
                others: others.iter().map(|b| Auth::from(*b)).collect(),
                after,
            })
            .await?
        {
            Reply::MembersChanged { members, .. } => Ok(members),
            _ => anyhow::bail!("unexpected candidate response"),
        }
    }
    pub async fn watch(&self, others: &[&SessionBinding], after: i64) -> Result<i64> {
        let operation = Operation::Watch {
            others: others.iter().map(|b| Auth::from(*b)).collect(),
            after,
        };
        match self.exchange(operation).await? {
            Reply::Changed { sequence } => Ok(sequence),
            _ => anyhow::bail!("unexpected candidate response"),
        }
    }
    pub fn new(path: &'a Path, binding: &'a SessionBinding) -> Self {
        Self { path, binding }
    }
    async fn call(&self, operation: Operation) -> Result<Reply> {
        tokio::time::timeout(REQUEST_TIMEOUT, self.exchange(operation))
            .await
            .context("candidate connector timed out")?
    }
    async fn exchange(&self, operation: Operation) -> Result<Reply> {
        let mut stream = tokio::time::timeout(REQUEST_TIMEOUT, UnixStream::connect(self.path))
            .await
            .context("candidate connection timed out")?
            .context("candidate connector unavailable")?;
        ensure!(
            stream.peer_cred()?.uid() == crate::effective_uid(),
            "candidate server rejected"
        );
        let mut bytes = serde_json::to_vec(&Request {
            v: 1,
            auth: self.binding.into(),
            request: operation,
        })?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() <= MAX_REQUEST_BYTES,
            "candidate request exceeds bound"
        );
        stream.write_all(&bytes).await?;
        let reply: Reply = serde_json::from_slice(&frame(&mut stream, MAX_REPLY_BYTES).await?)?;
        if let Reply::Error { message, .. } = reply {
            anyhow::bail!(message);
        }
        Ok(reply)
    }
    pub async fn send(&self, room: &str, key: &str, body: &str) -> Result<i64> {
        match self
            .call(Operation::Send {
                room: room.into(),
                key: key.into(),
                body: body.into(),
            })
            .await?
        {
            Reply::Sent { sequence } => Ok(sequence),
            _ => anyhow::bail!("unexpected candidate response"),
        }
    }
    pub async fn check(&self) -> Result<Option<Batch>> {
        match self.call(Operation::Check).await? {
            Reply::Empty => Ok(None),
            Reply::Batch { items, reference } => Ok(Some(Batch { items, reference })),
            _ => anyhow::bail!("unexpected candidate response"),
        }
    }
    pub async fn receipt(&self, batch: &Batch) -> Result<()> {
        match self
            .call(Operation::Receipt {
                reference: batch.reference.clone(),
            })
            .await?
        {
            Reply::Receipted => Ok(()),
            _ => anyhow::bail!("unexpected candidate response"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_nonreading_client_cannot_hold_a_reply_slot_forever() {
        use std::os::fd::AsRawFd;
        let (mut server, _nonreading_client) = UnixStream::pair().unwrap();
        let buffer: libc::c_int = 1024;
        let result = unsafe {
            libc::setsockopt(
                server.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                &buffer as *const _ as *const libc::c_void,
                std::mem::size_of_val(&buffer) as libc::socklen_t,
            )
        };
        assert_eq!(result, 0);
        let error = write_reply(
            &mut server,
            &vec![b'x'; MAX_REPLY_BYTES],
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("reply timed out"));
    }
}
