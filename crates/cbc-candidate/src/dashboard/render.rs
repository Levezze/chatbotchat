//! HTML for the dashboard. Every value that came from a member is agent text, so it passes
//! through [`esc`] on its way into the page, and the page carries no script at all.
use crate::requests::{Request, RequestOwner, MAX_REQUEST_PAGE};

/// One pending-requests panel: a context and one page of its waiting requests.
pub struct Panel {
    pub context: String,
    pub requests: Vec<Request>,
}

/// The one escape for text and attribute values alike.
pub fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

pub fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{}</title><style>{STYLE}</style></head><body><main>{body}</main></body></html>\n",
        esc(title)
    )
}

pub fn home(panels: &[Panel]) -> String {
    let mut body = String::from("<h1>CBC dashboard</h1><h2>Pending requests</h2>");
    if panels.is_empty() {
        body.push_str("<p class=\"quiet\">No contexts yet.</p>");
    }
    for panel in panels {
        body.push_str(&format!("<section><h3>{}</h3>", esc(&panel.context)));
        if panel.requests.is_empty() {
            body.push_str("<p class=\"quiet\">Nothing waiting.</p>");
        } else {
            body.push_str(
                "<table><thead><tr><th>Request</th><th>From</th><th>For</th><th>Item</th>\
                 <th>Question</th><th>State</th><th>Opened (UTC)</th></tr></thead><tbody>",
            );
            for r in &panel.requests {
                let owner = match &r.owner {
                    RequestOwner::User => "you".to_string(),
                    RequestOwner::Member(member) => member.clone(),
                };
                body.push_str(&format!(
                    "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td>\
                     <td class=\"text\">{}</td><td>{}</td><td>{}</td></tr>",
                    esc(&r.request),
                    esc(&r.callback),
                    esc(&owner),
                    esc(r.item.as_deref().unwrap_or("")),
                    esc(&r.question),
                    esc(&format!("{:?}", r.state).to_lowercase()),
                    utc(r.created_wall_ms),
                ));
            }
            body.push_str("</tbody></table>");
        }
        if panel.requests.len() == MAX_REQUEST_PAGE as usize {
            let last = &panel.requests[panel.requests.len() - 1].request;
            let href = format!("/?context={}&after={}", query(&panel.context), query(last));
            body.push_str(&format!("<p><a href=\"{}\">Next page</a></p>", esc(&href)));
        }
        body.push_str("</section>");
    }
    page("CBC dashboard", &body)
}

fn query(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// `YYYY-MM-DD HH:MM:SS` from Unix milliseconds, without a date library.
fn utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days, after Howard Hinnant's public-domain algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

const STYLE: &str =
    "body{font:15px/1.45 system-ui,sans-serif;margin:0;color:#1d1d1f;background:#fafafa}\
main{max-width:1100px;margin:0 auto;padding:16px}\
table{border-collapse:collapse;width:100%;background:#fff}\
th,td{border-bottom:1px solid #e3e3e3;padding:6px 8px;text-align:left;vertical-align:top}\
td.text{white-space:pre-wrap;overflow-wrap:anywhere}\
.quiet{color:#6e6e73}\
@media (prefers-color-scheme:dark){body{color:#f2f2f2;background:#161617}table{background:#1f1f21}\
th,td{border-color:#333}.quiet{color:#a1a1a6}}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_covers_every_markup_character() {
        assert_eq!(
            esc(r#"<a href="x" title='y'>&</a>"#),
            "&lt;a href=&quot;x&quot; title=&#39;y&#39;&gt;&amp;&lt;/a&gt;"
        );
    }

    #[test]
    fn utc_formats_known_instants() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(951_782_400_000), "2000-02-29 00:00:00");
        assert_eq!(utc(1_793_577_600_123), "2026-11-02 00:00:00");
    }
}
