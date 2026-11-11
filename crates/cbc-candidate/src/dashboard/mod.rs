//! The dashboard (T-11): a plain page the core serves on its own loopback port. It reads
//! through the same trusted owner reads as the owner route and is reached only through a
//! session that the user's own `dashboard` command starts (T-12). D1 is read-only.
pub mod render;
pub mod session;

use crate::{requests::MAX_REQUEST_PAGE, store::Store};
use anyhow::{ensure, Result};
use axum::{
    extract::{RawQuery, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use session::Sessions;
use std::{future::Future, net::Ipv4Addr, sync::Arc};
use tokio::net::TcpListener;

pub const COOKIE: &str = "cbc_dashboard";
/// How many contexts the first page lists, each with one page of its requests.
pub const MAX_CONTEXTS: u32 = 20;
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

struct Dashboard {
    store: Arc<Store>,
    sessions: Arc<Sessions>,
    host: String,
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
    });
    let app = Router::new()
        .route("/", get(home))
        .route("/login", get(login))
        .fallback(|| async { (StatusCode::NOT_FOUND, plain("Not found.")) })
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
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
        (header::REFERRER_POLICY, "no-referrer"),
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
    let cookie = format!("{COOKIE}={session}; HttpOnly; SameSite=Strict; Path=/");
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
        .filter_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
        .any(|token| d.sessions.valid(token))
}

async fn home(
    State(d): State<Arc<Dashboard>>,
    RawQuery(query): RawQuery,
    headers: axum::http::HeaderMap,
) -> Response {
    if !signed_in(&d, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            plain("Not signed in. Run the dashboard command to open a session."),
        )
            .into_response();
    }
    match panels(&d.store, param(&query, "context"), param(&query, "after")).await {
        Ok(panels) => axum::response::Html(render::home(&panels)).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, plain("Not found.")).into_response(),
    }
}

async fn panels(
    store: &Store,
    context: Option<String>,
    after: Option<String>,
) -> Result<Vec<render::Panel>> {
    let contexts = match context {
        Some(context) => {
            ensure!(store.context_exists(&context).await?, "unknown context");
            vec![context]
        }
        None => store.contexts("", MAX_CONTEXTS).await?,
    };
    let after = after.unwrap_or_default();
    let mut panels = Vec::with_capacity(contexts.len());
    for context in contexts {
        let requests = store
            .owner_pending_requests(&context, &after, MAX_REQUEST_PAGE)
            .await?;
        panels.push(render::Panel { context, requests });
    }
    Ok(panels)
}
