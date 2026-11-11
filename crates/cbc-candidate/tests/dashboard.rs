//! D1 (T-11, T-12): the dashboard skeleton. A plain page served by the core over loopback,
//! reached only through a browser session that the user's own command creates. These are
//! synthetic tests against a tempdir sandbox; they never open a browser and never count as a
//! native PASS.
use cbc_candidate::{
    bindings::{Role, SessionBinding},
    connector::{LocalClient, LocalConnector, LocalOwnerClient},
    requests::{RequestOwner, MAX_REQUEST_PAGE},
    store::Store,
    Sandbox,
};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

struct Running {
    temp: tempfile::TempDir,
    socket: std::path::PathBuf,
    owner_key: String,
    addr: SocketAddr,
    worker: SessionBinding,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Running {
    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.addr.port())
    }
    fn owner(&self) -> LocalOwnerClient<'_> {
        LocalOwnerClient::new(&self.socket, &self.owner_key)
    }
    fn worker(&self) -> LocalClient<'_> {
        LocalClient::new(&self.socket, &self.worker)
    }
    async fn get(&self, path: &str, cookie: Option<&str>) -> Response {
        http(self.addr, "GET", path, &self.host(), cookie).await
    }
    /// A signed-in browser: mint a code on the owner route and redeem it.
    async fn login(&self) -> String {
        let code = self.owner().dashboard_login_code().await.unwrap();
        let response = self.get(&format!("/login?code={code}"), None).await;
        assert_eq!(response.status, 303);
        assert!(response
            .header("set-cookie")
            .unwrap()
            .starts_with(&format!("cbc_dashboard_{}=", self.addr.port())));
        session_cookie(&response).expect("a session cookie")
    }
}

async fn running(with_dashboard: bool) -> Running {
    running_with(with_dashboard, &[]).await
}

async fn running_with(with_dashboard: bool, extra_contexts: &[&str]) -> Running {
    let temp = tempfile::Builder::new()
        .prefix("cbc-dash-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8996,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for context in extra_contexts {
        store.create_context(context).await.unwrap();
    }
    store
        .register_member("project", "worker", Role::FeatureLead)
        .await
        .unwrap();
    let worker = store
        .bind_member("project", "worker", "device", "w")
        .await
        .unwrap();
    let mut connector = LocalConnector::bind(&sandbox, store).unwrap();
    let mut addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    if with_dashboard {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        addr = listener.local_addr().unwrap();
        connector = connector.with_dashboard(listener).unwrap();
    }
    let socket = connector.path().to_owned();
    let owner_key = std::fs::read_to_string(sandbox.root().join("owner.key"))
        .unwrap()
        .trim_end()
        .to_owned();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(connector.run(async {
        let _ = stopped.await;
    }));
    Running {
        temp,
        socket,
        owner_key,
        addr,
        worker,
        stop,
        task,
    }
}

async fn stop(case: Running) {
    case.stop.send(()).unwrap();
    case.task.await.unwrap().unwrap();
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A raw HTTP/1.1 exchange, so a test controls every header, Host included.
async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    host: &str,
    cookie: Option<&str>,
) -> Response {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let cookie = cookie
        .map(|c| format!("Cookie: cbc_dashboard_{}={c}\r\n", addr.port()))
        .unwrap_or_default();
    stream
        .write_all(
            format!(
                "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{cookie}Connection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8(raw).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .map(|l| {
            let (n, v) = l.split_once(':').unwrap();
            (n.trim().to_string(), v.trim().to_string())
        })
        .collect();
    Response {
        status,
        headers,
        body: body.to_string(),
    }
}

/// The session cookie is named for the port, so two cores in one browser profile don't sign
/// each other out: cookies are scoped by host, not port.
fn session_cookie(response: &Response) -> Option<String> {
    let set = response.header("set-cookie")?;
    let (name, rest) = set.split_once('=')?;
    let port = name.strip_prefix("cbc_dashboard_")?;
    assert!(port.parse::<u16>().is_ok(), "cookie name {name}");
    let value = rest.split(';').next()?;
    Some(value.to_string())
}

#[tokio::test]
async fn a_single_use_code_becomes_a_locked_down_session_cookie() {
    let case = running(true).await;
    let code = case.owner().dashboard_login_code().await.unwrap();
    assert_eq!(code.len(), 32);

    let login = case.get(&format!("/login?code={code}"), None).await;
    assert_eq!(login.status, 303);
    assert_eq!(
        login.header("location"),
        Some("/"),
        "the code leaves the address bar"
    );
    let set = login.header("set-cookie").unwrap();
    for attribute in ["HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(set.contains(attribute), "cookie lacks {attribute}: {set}");
    }
    let cookie = session_cookie(&login).unwrap();
    assert!(
        !cookie.contains(&code),
        "the session is not derived from the code"
    );
    assert_eq!(cookie.len(), 64);

    let page = case.get("/", Some(&cookie)).await;
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Pending requests"));

    let reused = case.get(&format!("/login?code={code}"), None).await;
    assert_eq!(reused.status, 403, "a code works once");
    assert!(reused.header("set-cookie").is_none());

    for cookie in [None, Some("0".repeat(64).as_str()), Some(code.as_str())] {
        let refused = case.get("/", cookie).await;
        assert_eq!(refused.status, 401);
        assert!(!refused.body.contains("Pending requests"));
    }
    assert!(LocalOwnerClient::new(&case.socket, "not-an-owner-key")
        .dashboard_login_code()
        .await
        .is_err());
    stop(case).await;
}

/// Every response, whatever its status, forbids scripts, framing, referrers and caching.
#[tokio::test]
async fn every_response_carries_the_lockdown_headers() {
    let case = running(true).await;
    let cookie = case.login().await;
    let code = case.owner().dashboard_login_code().await.unwrap();
    let responses = [
        case.get("/", Some(&cookie)).await,
        case.get("/", None).await,
        case.get("/login?code=nope", None).await,
        case.get(&format!("/login?code={code}"), None).await,
        case.get("/missing", Some(&cookie)).await,
        http(case.addr, "GET", "/", "evil.example", Some(&cookie)).await,
    ];
    let statuses: Vec<u16> = responses.iter().map(|r| r.status).collect();
    assert_eq!(statuses, vec![200, 401, 403, 303, 404, 421]);
    for response in &responses {
        assert_eq!(response.header("content-security-policy"), Some(CSP));
        assert_eq!(response.header("referrer-policy"), Some("no-referrer"));
        assert_eq!(response.header("cache-control"), Some("no-store"));
        assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
        assert!(!response.body.to_ascii_lowercase().contains("<script"));
    }
    stop(case).await;
}

/// A request whose Host is not this loopback address is refused, signed in or not, so a
/// rebound DNS name can't reach the page.
#[tokio::test]
async fn a_foreign_host_is_refused() {
    let case = running(true).await;
    let cookie = case.login().await;
    case.worker()
        .open_request(&RequestOwner::User, None, "Ship it?", "k-1")
        .await
        .unwrap();
    let port = case.addr.port();
    for host in [
        format!("localhost:{port}"),
        "127.0.0.1".to_string(),
        format!("127.0.0.1:{}", port.wrapping_add(1)),
        format!("attacker.example:{port}"),
    ] {
        let response = http(case.addr, "GET", "/", &host, Some(&cookie)).await;
        assert_eq!(response.status, 421, "host {host}");
        assert!(!response.body.contains("Ship it?"));
    }
    // Two Host headers, one of them right, are still refused.
    let doubled = http(
        case.addr,
        "GET",
        "/",
        &format!("{}\r\nHost: attacker.example", case.host()),
        Some(&cookie),
    )
    .await;
    assert_eq!(doubled.status, 421);
    assert!(!doubled.body.contains("Ship it?"));
    // An absolute-form target names its own authority; it must match too.
    let absolute = http(
        case.addr,
        "GET",
        &format!("http://attacker.example:{port}/"),
        &case.host(),
        Some(&cookie),
    )
    .await;
    assert_eq!(absolute.status, 421);
    assert!(!absolute.body.contains("Ship it?"));
    assert_eq!(case.get("/", Some(&cookie)).await.status, 200);
    stop(case).await;
}

/// Agent text is data. A hostile question renders escaped, never as markup.
#[tokio::test]
async fn agent_text_is_escaped() {
    let case = running(true).await;
    let cookie = case.login().await;
    let hostile = r#"<script>alert(1)</script><img src=x onerror=alert(2)> "q" 'a' & done"#;
    case.worker()
        .open_request(&RequestOwner::User, None, hostile, "k-1")
        .await
        .unwrap();
    let page = case.get("/", Some(&cookie)).await;
    assert_eq!(page.status, 200);
    assert!(!page.body.contains("<script"));
    assert!(!page.body.contains("<img"));
    assert!(page.body.contains(
        "&lt;script&gt;alert(1)&lt;/script&gt;&lt;img src=x onerror=alert(2)&gt; &quot;q&quot; &#39;a&#39; &amp; done"
    ));
    stop(case).await;
}

/// The pending panel shows one bounded page per context, with a link to the next.
#[tokio::test]
async fn pending_requests_come_in_pages() {
    let case = running(true).await;
    let cookie = case.login().await;
    let mut ids = Vec::new();
    for n in 0..=MAX_REQUEST_PAGE {
        ids.push(
            case.worker()
                .open_request(
                    &RequestOwner::User,
                    None,
                    &format!("Question {n}"),
                    &format!("k-{n}"),
                )
                .await
                .unwrap()
                .request,
        );
    }
    ids.sort();
    let first = case.get("/", Some(&cookie)).await;
    let shown = |body: &str| ids.iter().filter(|id| body.contains(id.as_str())).count();
    assert_eq!(shown(&first.body), MAX_REQUEST_PAGE as usize);
    let next = format!(
        "/?context=project&amp;after={}",
        ids[MAX_REQUEST_PAGE as usize - 1]
    );
    assert!(first.body.contains(&next), "a next-page link");
    let second = case.get(&next.replace("&amp;", "&"), Some(&cookie)).await;
    assert_eq!(second.status, 200);
    assert_eq!(shown(&second.body), 1);
    assert!(second.body.contains(ids.last().unwrap().as_str()));
    stop(case).await;
}

/// T-12: neither the login code nor the session is written anywhere in the state directory,
/// the database bytes included.
#[tokio::test]
async fn no_dashboard_secret_reaches_the_state_directory() {
    let case = running(true).await;
    let code = case.owner().dashboard_login_code().await.unwrap();
    let login = case.get(&format!("/login?code={code}"), None).await;
    let cookie = session_cookie(&login).unwrap();
    case.worker()
        .open_request(&RequestOwner::User, None, "Ship it?", "k-1")
        .await
        .unwrap();
    assert_eq!(case.get("/", Some(&cookie)).await.status, 200);
    let mut files = 0;
    let mut stack = vec![case.temp.path().to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                files += 1;
                let bytes = std::fs::read(&path).unwrap();
                for secret in [&code, &cookie] {
                    assert!(
                        !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                        "a dashboard secret was written to {}",
                        path.display()
                    );
                }
            }
        }
    }
    assert!(files >= 3, "the scan saw the database and its siblings");
    stop(case).await;
}

#[tokio::test]
async fn a_core_without_a_dashboard_mints_no_code() {
    let case = running(false).await;
    assert!(case.owner().dashboard_login_code().await.is_err());
    stop(case).await;
}

/// The dashboard binds the IPv4 loopback address only.
#[tokio::test]
async fn a_dashboard_listener_off_loopback_is_refused() {
    let temp = tempfile::Builder::new()
        .prefix("cbc-dash-")
        .tempdir_in("/tmp")
        .unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8996,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    let connector = LocalConnector::bind(&sandbox, store).unwrap();
    let wide = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    assert!(connector.with_dashboard(wide).is_err());
}

/// A cookie for another core's port is not this core's session.
#[tokio::test]
async fn a_cookie_named_for_another_port_is_not_a_session() {
    let case = running(true).await;
    let cookie = case.login().await;
    let wrong = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nCookie: cbc_dashboard_{}={cookie}; cbc_dashboard={cookie}\r\nConnection: close\r\n\r\n",
        case.host(),
        case.addr.port().wrapping_add(1)
    );
    let mut stream = tokio::net::TcpStream::connect(case.addr).await.unwrap();
    stream.write_all(wrong.as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    assert!(raw.starts_with("HTTP/1.1 401"), "{raw}");
    stop(case).await;
}

/// Past the first page of contexts, a link leads on, so every context's requests are reachable.
#[tokio::test]
async fn contexts_come_in_pages() {
    let extra: Vec<String> = (0..cbc_candidate::dashboard::MAX_CONTEXTS)
        .map(|n| format!("ctx-{n:02}"))
        .collect();
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    let case = running_with(true, &extra).await;
    let cookie = case.login().await;
    case.worker()
        .open_request(&RequestOwner::User, None, "In project?", "k-1")
        .await
        .unwrap();
    let first = case.get("/", Some(&cookie)).await;
    assert_eq!(first.status, 200);
    assert!(first.body.contains("ctx-00"));
    assert!(
        !first.body.contains("In project?"),
        "\"project\" sorts after the first page"
    );
    let last = format!("ctx-{:02}", cbc_candidate::dashboard::MAX_CONTEXTS - 1);
    let link = format!("/?contexts_after={last}");
    assert!(first.body.contains(&link), "a link to more contexts");
    let second = case.get(&link, Some(&cookie)).await;
    assert_eq!(second.status, 200);
    assert!(second.body.contains("In project?"));
    assert!(!second.body.contains("ctx-00"));
    assert!(
        !second.body.contains("contexts_after"),
        "the last page links nowhere"
    );
    stop(case).await;
}

/// Exactly a page of contexts has no next page.
#[tokio::test]
async fn a_full_last_page_of_contexts_links_nowhere() {
    let extra: Vec<String> = (1..cbc_candidate::dashboard::MAX_CONTEXTS)
        .map(|n| format!("ctx-{n:02}"))
        .collect();
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    let case = running_with(true, &extra).await;
    let cookie = case.login().await;
    let page = case.get("/", Some(&cookie)).await;
    assert_eq!(page.status, 200);
    assert!(page.body.contains("project"));
    assert!(!page.body.contains("contexts_after"));
    stop(case).await;
}

/// A browser that stops reading can't keep the core from releasing its store on shutdown.
#[tokio::test]
async fn shutdown_closes_a_connection_that_stopped_reading() {
    let case = running(true).await;
    let cookie = case.login().await;
    let largest = "\u{1F600}".repeat(4000);
    for n in 0..MAX_REQUEST_PAGE {
        case.worker()
            .open_request(&RequestOwner::User, None, &largest, &format!("k-{n}"))
            .await
            .unwrap();
    }
    // Pipeline far more page bytes than the socket buffers hold, and never read any of them.
    let mut stalled = tokio::net::TcpStream::connect(case.addr).await.unwrap();
    let one = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nCookie: cbc_dashboard_{}={cookie}\r\n\r\n",
        case.host(),
        case.addr.port()
    );
    stalled.write_all(one.repeat(200).as_bytes()).await.unwrap();
    // A second connection that sent half its headers.
    let mut half = tokio::net::TcpStream::connect(case.addr).await.unwrap();
    half.write_all(b"GET / HTTP/1.1\r\nHost: ").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    tokio::time::timeout(std::time::Duration::from_secs(3), stop(case))
        .await
        .expect("shutdown finishes promptly and releases the store");
    drop((stalled, half));
}
