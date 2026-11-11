//! The dashboard (T-11): a plain page the core serves on its own loopback port. It reads
//! through the same trusted owner reads as the owner route and is reached only through a
//! session that the user's own `dashboard` command starts (T-12). Its one write is the user's
//! answer to a question for the user (D2), recorded as an owner answer.
pub mod render;
pub mod session;

use crate::{requests::MAX_REQUEST_PAGE, store::Store};
use anyhow::{ensure, Context as _, Result};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, RawQuery, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use session::Sessions;
use std::{future::Future, net::Ipv4Addr, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::Semaphore, task::JoinSet};

/// The session cookie is `cbc_dashboard_<port>`: browsers scope cookies by host, not port,
/// so a shared name would let two local cores sign each other out.
pub const COOKIE_PREFIX: &str = "cbc_dashboard_";
/// How many contexts the first page lists, each with one page of its requests.
pub const MAX_CONTEXTS: u32 = 20;
/// Work items per context page, in item ID order.
pub const MAX_WORK_PAGE: u32 = 20;
/// Outbox events per context page, newest first.
pub const MAX_OUTBOX_PAGE: u32 = 20;
/// Open browser connections at once; past this a new one is closed unanswered.
const MAX_CONNECTIONS: usize = 16;
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// An answer form: 4000 characters of up to four UTF-8 bytes each, percent-encoded, with room
/// for the hidden fields.
const MAX_FORM_BYTES: usize = 64 * 1024;
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

struct Dashboard {
    store: Arc<Store>,
    sessions: Arc<Sessions>,
    host: String,
    cookie: String,
}

/// Only the IPv4 loopback address, so the Host check has one right answer.
pub fn check_listener(listener: &TcpListener) -> Result<()> {
    ensure!(
        listener.local_addr()?.ip() == Ipv4Addr::LOCALHOST,
        "the dashboard listens on 127.0.0.1 only"
    );
    Ok(())
}

pub async fn serve(
    listener: TcpListener,
    store: Arc<Store>,
    sessions: Arc<Sessions>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    check_listener(&listener)?;
    let state = Arc::new(Dashboard {
        store,
        sessions,
        host: format!("127.0.0.1:{}", listener.local_addr()?.port()),
        cookie: format!("{COOKIE_PREFIX}{}", listener.local_addr()?.port()),
    });
    let app = Router::new()
        .route("/", get(home))
        .route("/login", get(login))
        .route("/item", get(item))
        .route("/timeline", get(timeline))
        .route(
            "/answer",
            post(answer).layer(DefaultBodyLimit::max(MAX_FORM_BYTES)),
        )
        .fallback(|| async { (StatusCode::NOT_FOUND, plain("Not found.")) })
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state);
    // Each connection is a task this loop owns, so shutdown can close it even while a browser
    // has stopped reading, and nothing outlives the loop holding the store.
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut connections = JoinSet::new();
    let mut backoff = Duration::from_millis(25);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            _ = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept() => {
                // The socket listener's policy: back off from a transient failure, stop on any other.
                let stream = match accepted {
                    Ok((stream, _)) => {
                        backoff = Duration::from_millis(25);
                        stream
                    }
                    Err(error) if crate::connector::transient_accept(&error) => {
                        eprintln!("dashboard accept temporarily unavailable: {}", error.kind());
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(1));
                        continue;
                    }
                    Err(error) => {
                        connections.abort_all();
                        return Err(error.into());
                    }
                };
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    continue;
                };
                let service = TowerToHyperService::new(app.clone());
                connections.spawn(async move {
                    let _permit = permit;
                    let _ = hyper::server::conn::http1::Builder::new()
                        .timer(TokioTimer::new())
                        .header_read_timeout(HEADER_READ_TIMEOUT)
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        }
    }
    drop(listener);
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    Ok(())
}

/// Every response: refuse a foreign Host before anything else runs, then lock the page down.
async fn guard(State(d): State<Arc<Dashboard>>, request: Request, next: Next) -> Response {
    let mut hosts = request.headers().get_all(header::HOST).iter();
    let host_ok = matches!((hosts.next(), hosts.next()), (Some(h), None) if h == d.host.as_str())
        && request
            .uri()
            .authority()
            .is_none_or(|a| a.as_str() == d.host);
    let mut response = if host_ok {
        next.run(request).await
    } else {
        (StatusCode::MISDIRECTED_REQUEST, plain("Unknown host.")).into_response()
    };
    let headers = response.headers_mut();
    for (name, value) in [
        (header::CONTENT_SECURITY_POLICY, CSP),
        // Not no-referrer: under it a browser sends `Origin: null` on the page's own form post,
        // and the answer route could not tell its own page from any other. same-origin still
        // sends no referrer off this origin.
        (header::REFERRER_POLICY, "same-origin"),
        (header::CACHE_CONTROL, "no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    response
}

fn param(query: &Option<String>, name: &str) -> Option<String> {
    url::form_urlencoded::parse(query.as_deref()?.as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

fn plain(text: &str) -> axum::response::Html<String> {
    axum::response::Html(render::page(
        "CBC dashboard",
        &format!("<p>{}</p>", render::esc(text)),
    ))
}

/// Spend a login code for a session cookie, then send the browser on without the code.
async fn login(State(d): State<Arc<Dashboard>>, RawQuery(query): RawQuery) -> Response {
    let Some(session) = param(&query, "code").and_then(|code| d.sessions.redeem(&code)) else {
        return (
            StatusCode::FORBIDDEN,
            plain("This sign-in link has expired or was already used. Run the dashboard command again."),
        )
            .into_response();
    };
    let cookie = format!("{}={session}; HttpOnly; SameSite=Strict; Path=/", d.cookie);
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/".to_string()),
            (header::SET_COOKIE, cookie),
        ],
    )
        .into_response()
}

fn signed_in(d: &Dashboard, request: &axum::http::HeaderMap) -> bool {
    request
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| {
            pair.trim()
                .strip_prefix(d.cookie.as_str())?
                .strip_prefix('=')
        })
        .any(|token| d.sessions.valid(token))
}

fn signed_out() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        plain("Not signed in. Run the dashboard command to open a session."),
    )
        .into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, plain("Not found.")).into_response()
}

/// A page, or a 404 for anything missing, hidden or malformed alike.
fn html(page: Result<String>) -> Response {
    match page {
        Ok(page) => axum::response::Html(page).into_response(),
        Err(_) => not_found(),
    }
}

async fn home(
    State(d): State<Arc<Dashboard>>,
    RawQuery(query): RawQuery,
    headers: axum::http::HeaderMap,
) -> Response {
    if !signed_in(&d, &headers) {
        return signed_out();
    }
    html(home_page(&d.store, &query).await)
}

async fn home_page(store: &Store, query: &Option<String>) -> Result<String> {
    let outbox_before = param(query, "outbox_before")
        .map(|v| v.parse::<i64>())
        .transpose()?;
    let pages = Pages {
        items_after: param(query, "items_after").unwrap_or_default(),
        requests_after: param(query, "after").unwrap_or_default(),
        outbox_before,
    };
    let (contexts, more) = match param(query, "context") {
        Some(context) => {
            ensure!(store.context_exists(&context).await?, "unknown context");
            (vec![context], None)
        }
        None => {
            let after = param(query, "contexts_after").unwrap_or_default();
            let mut contexts = store.contexts(&after, MAX_CONTEXTS + 1).await?;
            let more = one_past(&mut contexts, MAX_CONTEXTS).cloned();
            (contexts, more)
        }
    };
    let mut panels = Vec::with_capacity(contexts.len());
    for context in contexts {
        panels.push(panel(store, context, &pages).await?);
    }
    Ok(render::home(&panels, more.as_deref()))
}

/// Where each of a context's lists starts.
struct Pages {
    items_after: String,
    requests_after: String,
    outbox_before: Option<i64>,
}

/// One past the page says whether a next page exists, so no link leads to an empty one.
/// Trims the extra entry and returns the last one kept, when more follow.
fn one_past<T>(entries: &mut Vec<T>, page: u32) -> Option<&T> {
    if entries.len() > page as usize {
        entries.truncate(page as usize);
        entries.last()
    } else {
        None
    }
}

async fn panel(store: &Store, context: String, pages: &Pages) -> Result<render::Panel> {
    let mut items = store
        .work_items(&context, &pages.items_after, MAX_WORK_PAGE + 1)
        .await?;
    let items_more = one_past(&mut items, MAX_WORK_PAGE).map(|i| i.item.clone());
    let requests = store
        .owner_pending_requests(&context, &pages.requests_after, MAX_REQUEST_PAGE)
        .await?;
    let mut outbox = store
        .outbox_timelines(&context, pages.outbox_before, MAX_OUTBOX_PAGE + 1)
        .await?;
    let outbox_more = one_past(&mut outbox, MAX_OUTBOX_PAGE).map(|e| e.sequence);
    Ok(render::Panel {
        context,
        items,
        items_more,
        requests,
        outbox,
        outbox_more,
    })
}

/// Revisions per page on a work item's page, oldest first.
const REVISION_PAGE: u32 = 50;

async fn item(
    State(d): State<Arc<Dashboard>>,
    RawQuery(query): RawQuery,
    headers: axum::http::HeaderMap,
) -> Response {
    if !signed_in(&d, &headers) {
        return signed_out();
    }
    html(item_page(&d.store, &query).await)
}

async fn item_page(store: &Store, query: &Option<String>) -> Result<String> {
    let context = param(query, "context").context("no context")?;
    let id = param(query, "item").context("no item")?;
    let after = param(query, "after")
        .map(|v| v.parse::<i64>())
        .transpose()?
        .unwrap_or(0);
    let item = store
        .work_item(&context, &id)
        .await?
        .context("unknown item")?;
    let mut revisions = store
        .work_revision_page(&context, &id, after, REVISION_PAGE + 1)
        .await?;
    let more = one_past(&mut revisions, REVISION_PAGE).map(|r| r.revision);
    Ok(render::work_item(&context, &item, &revisions, more))
}

async fn timeline(
    State(d): State<Arc<Dashboard>>,
    RawQuery(query): RawQuery,
    headers: axum::http::HeaderMap,
) -> Response {
    if !signed_in(&d, &headers) {
        return signed_out();
    }
    html(timeline_page(&d.store, &query).await)
}

async fn timeline_page(store: &Store, query: &Option<String>) -> Result<String> {
    let context = param(query, "context").context("no context")?;
    let sequence = param(query, "sequence")
        .context("no sequence")?
        .parse::<i64>()?;
    let event = store
        .delivery_timeline(&context, sequence)
        .await?
        .context("unknown event")?;
    Ok(render::timeline(&context, &event))
}

/// The page's own form post and nothing else: the session, then this origin exactly. The
/// session cookie is SameSite=Strict as well; the Origin check does not depend on it.
async fn answer(
    State(d): State<Arc<Dashboard>>,
    headers: axum::http::HeaderMap,
    form: Bytes,
) -> Response {
    if !signed_in(&d, &headers) {
        return signed_out();
    }
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let expected = format!("http://{}", d.host);
    if !matches!((origins.next(), origins.next()), (Some(o), None) if o == expected.as_str()) {
        return (
            StatusCode::FORBIDDEN,
            plain("This answer did not come from this dashboard's own page."),
        )
            .into_response();
    }
    let field = |name: &str| {
        url::form_urlencoded::parse(&form)
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    let (Some(context), Some(request), Some(key), Some(body)) = (
        field("context"),
        field("request"),
        field("key"),
        field("body"),
    ) else {
        return not_found();
    };
    match d
        .store
        .dashboard_answer_request(&context, &request, &body, &key)
        .await
    {
        Ok(_) => (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, format!("/?context={}", query(&context)))],
        )
            .into_response(),
        Err(error) => refused(&error),
    }
}

/// A missing or malformed request is not found, like any missing page. Anything else is a
/// refusal the user should read: already answered, withdrawn, a member's question, bad text.
fn refused(error: &anyhow::Error) -> Response {
    let message = error.to_string();
    let missing = message == "unknown request"
        || [
            "invalid context",
            "invalid request id",
            "invalid request key",
        ]
        .iter()
        .any(|p| message.starts_with(p));
    if missing {
        return not_found();
    }
    (
        StatusCode::CONFLICT,
        plain(&format!("Not recorded: {message}")),
    )
        .into_response()
}

fn query(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
