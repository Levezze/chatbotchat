//! HTML for the dashboard. Every value that came from a member is agent text, so it passes
//! through [`esc`] on its way into the page, and the page carries no script at all.
use crate::{
    facts::{Interval, ItemTimeline, Stamp},
    requests::{Request, RequestOwner, MAX_REQUEST_PAGE},
    work::{Actor, WorkItem, WorkRevision},
};

/// One context's panel: a page each of its work items, waiting requests and outbox events.
pub struct Panel {
    pub context: String,
    pub items: Vec<WorkItem>,
    /// The last item shown, when more follow it.
    pub items_more: Option<String>,
    pub requests: Vec<Request>,
    pub outbox: Vec<ItemTimeline>,
    /// The oldest event shown, when older ones exist.
    pub outbox_more: Option<i64>,
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

/// `more` is the last context shown when more follow it.
pub fn home(panels: &[Panel], more: Option<&str>) -> String {
    let mut body = String::from("<h1>CBC dashboard</h1>");
    if panels.is_empty() {
        body.push_str("<p class=\"quiet\">No contexts yet.</p>");
    }
    for panel in panels {
        let context = &panel.context;
        body.push_str(&format!("<section><h2>{}</h2>", esc(context)));
        work_table(&mut body, context, &panel.items);
        if let Some(last) = &panel.items_more {
            link(
                &mut body,
                &format!("/?context={}&items_after={}", query(context), query(last)),
                "More work items",
            );
        }
        requests_table(&mut body, context, &panel.requests);
        outbox_table(&mut body, context, &panel.outbox);
        if let Some(oldest) = panel.outbox_more {
            link(
                &mut body,
                &format!("/?context={}&outbox_before={oldest}", query(context)),
                "Older events",
            );
        }
        body.push_str("</section>");
    }
    if let Some(last) = more {
        link(
            &mut body,
            &format!("/?contexts_after={}", query(last)),
            "More contexts",
        );
    }
    page("CBC dashboard", &body)
}

fn link(body: &mut String, href: &str, text: &str) {
    body.push_str(&format!(
        "<p><a href=\"{}\">{}</a></p>",
        esc(href),
        esc(text)
    ));
}

fn anchor(href: &str, text: &str) -> String {
    format!("<a href=\"{}\">{}</a>", esc(href), esc(text))
}

fn item_href(context: &str, item: &str) -> String {
    format!("/item?context={}&item={}", query(context), query(item))
}

fn timeline_href(context: &str, sequence: i64) -> String {
    format!("/timeline?context={}&sequence={sequence}", query(context))
}

fn table(body: &mut String, headings: &[&str]) {
    body.push_str("<table><thead><tr>");
    for heading in headings {
        body.push_str(&format!("<th>{}</th>", esc(heading)));
    }
    body.push_str("</tr></thead><tbody>");
}

fn row(body: &mut String, cells: &[String]) {
    body.push_str("<tr>");
    for cell in cells {
        body.push_str(&format!("<td>{cell}</td>"));
    }
    body.push_str("</tr>");
}

fn work_table(body: &mut String, context: &str, items: &[WorkItem]) {
    body.push_str("<h3>Work items</h3>");
    if items.is_empty() {
        body.push_str("<p class=\"quiet\">No work items.</p>");
        return;
    }
    table(
        body,
        &[
            "Item",
            "Owner",
            "Stage",
            "Summary",
            "Next action",
            "Blocked",
            "Branch",
            "HITL",
            "Updated (UTC)",
        ],
    );
    for item in items {
        row(
            body,
            &[
                anchor(&item_href(context, &item.item), &item.item),
                esc(&item.owner),
                esc(item.stage.as_str()),
                text(&item.summary),
                text(&item.next_action),
                esc(item.blocked_operation.as_deref().unwrap_or("")),
                esc(item.branch.as_deref().unwrap_or("")),
                if item.hitl { "on" } else { "" }.into(),
                utc(item.updated_wall_ms),
            ],
        );
    }
    body.push_str("</tbody></table>");
}

/// Agent text that may hold line breaks.
fn text(value: &str) -> String {
    format!("<span class=\"text\">{}</span>", esc(value))
}

/// A question for the user gets a form; a question for a member is that member's to answer.
/// Each render carries a fresh key, so submitting the same form twice is one answer.
fn answer_form(context: &str, request: &Request) -> String {
    if let RequestOwner::Member(member) = &request.owner {
        return format!("<span class=\"quiet\">{}'s to answer</span>", esc(member));
    }
    let hidden = |name: &str, value: &str| {
        format!(
            "<input type=\"hidden\" name=\"{name}\" value=\"{}\">",
            esc(value)
        )
    };
    format!(
        "<form method=\"post\" action=\"/answer\">{}{}{}<textarea name=\"body\" rows=\"3\" maxlength=\"{}\" required></textarea><button>Answer</button></form>",
        hidden("context", context),
        hidden("request", &request.request),
        hidden("key", &super::session::form_key()),
        crate::requests::MAX_TEXT_CHARS,
    )
}

fn requests_table(body: &mut String, context: &str, requests: &[Request]) {
    body.push_str("<h3>Pending requests</h3>");
    if requests.is_empty() {
        body.push_str("<p class=\"quiet\">Nothing waiting.</p>");
        return;
    }
    table(
        body,
        &[
            "Request",
            "From",
            "For",
            "Item",
            "Question",
            "State",
            "Opened (UTC)",
            "Answer",
        ],
    );
    for r in requests {
        let owner = match &r.owner {
            RequestOwner::User => "you".to_string(),
            RequestOwner::Member(member) => member.clone(),
        };
        row(
            body,
            &[
                format!("<code>{}</code>", esc(&r.request)),
                esc(&r.callback),
                esc(&owner),
                esc(r.item.as_deref().unwrap_or("")),
                text(&r.question),
                esc(&format!("{:?}", r.state).to_lowercase()),
                utc(r.created_wall_ms),
                answer_form(context, r),
            ],
        );
    }
    body.push_str("</tbody></table>");
    if requests.len() == MAX_REQUEST_PAGE as usize {
        let last = &requests[requests.len() - 1].request;
        link(
            body,
            &format!("/?context={}&after={}", query(context), query(last)),
            "Next page",
        );
    }
}

fn outbox_table(body: &mut String, context: &str, events: &[ItemTimeline]) {
    body.push_str("<h3>Outbox</h3>");
    if events.is_empty() {
        body.push_str("<p class=\"quiet\">No events yet.</p>");
        return;
    }
    table(
        body,
        &["Event", "Class", "Room", "Committed (UTC)", "Receipted"],
    );
    for event in events {
        let receipted = event
            .recipients
            .iter()
            .filter(|r| r.receipted.is_some())
            .count();
        row(
            body,
            &[
                anchor(
                    &timeline_href(context, event.sequence),
                    &format!("#{}", event.sequence),
                ),
                esc(event.class.map_or("", |c| c.as_str())),
                esc(&event.room),
                committed(event.committed.as_ref()),
                format!("{receipted} of {}", event.recipients.len()),
            ],
        );
    }
    body.push_str("</tbody></table>");
}

fn stamp(at: Option<&Stamp>) -> String {
    at.map_or_else(|| "not yet".into(), |s| utc(s.wall_ms))
}

/// A commit always happened; a missing stamp means it was not recorded, not that it is pending.
fn committed(at: Option<&Stamp>) -> String {
    at.map_or_else(|| "unknown".into(), |s| utc(s.wall_ms))
}

fn interval(span: Option<&Interval>) -> String {
    span.map_or_else(
        || "&#8212;".into(),
        |i| {
            format!(
                "{:.3} ms ({})",
                i.micros as f64 / 1000.0,
                format!("{:?}", i.basis).to_lowercase()
            )
        },
    )
}

/// `more` is the last revision shown when more follow it.
pub fn work_item(
    context: &str,
    item: &WorkItem,
    revisions: &[WorkRevision],
    more: Option<i64>,
) -> String {
    let mut body = format!(
        "<p>{}</p><h1>Work item {}</h1>",
        anchor(
            &format!("/?context={}", query(context)),
            &format!("Back to {context}")
        ),
        esc(&item.item)
    );
    table(&mut body, &["Field", "Value"]);
    for (name, value) in [
        ("Context", esc(context)),
        ("Owner", esc(&item.owner)),
        ("Stage", esc(item.stage.as_str())),
        ("Summary", text(&item.summary)),
        ("Next action", text(&item.next_action)),
        (
            "Blocked operation",
            esc(item.blocked_operation.as_deref().unwrap_or("")),
        ),
        ("Branch", esc(item.branch.as_deref().unwrap_or(""))),
        ("HITL", if item.hitl { "on" } else { "off" }.into()),
        ("Revision", item.revision.to_string()),
        ("Created (UTC)", utc(item.created_wall_ms)),
        ("Updated (UTC)", utc(item.updated_wall_ms)),
    ] {
        row(&mut body, &[esc(name), value]);
    }
    body.push_str("</tbody></table><h2>Revisions</h2>");
    table(
        &mut body,
        &["Revision", "By", "Changed", "At (UTC)", "Delivered to"],
    );
    for revision in revisions {
        let by = match &revision.actor {
            Actor::Member(member) => esc(member),
            Actor::Owner => "you".into(),
        };
        let delivered = if revision.delivered_to.is_empty() {
            "nobody".to_string()
        } else {
            revision
                .delivered_to
                .iter()
                .zip(&revision.event_sequences)
                .map(|(room, sequence)| {
                    format!(
                        "{} ({})",
                        esc(room),
                        anchor(&timeline_href(context, *sequence), &format!("#{sequence}"))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        row(
            &mut body,
            &[
                revision.revision.to_string(),
                by,
                esc(&revision.changed.join(", ")),
                utc(revision.at_wall_ms),
                delivered,
            ],
        );
    }
    body.push_str("</tbody></table>");
    if let Some(last) = more {
        link(
            &mut body,
            &format!("{}&after={last}", item_href(context, &item.item)),
            "Later revisions",
        );
    }
    page(&format!("Work item {}", item.item), &body)
}

pub fn timeline(context: &str, event: &ItemTimeline) -> String {
    let mut body = format!(
        "<p>{}</p><h1>Event #{}</h1>",
        anchor(
            &format!("/?context={}", query(context)),
            &format!("Back to {context}")
        ),
        event.sequence
    );
    table(&mut body, &["Field", "Value"]);
    for (name, value) in [
        ("Context", esc(context)),
        ("Room", esc(&event.room)),
        ("Class", esc(event.class.map_or("unknown", |c| c.as_str()))),
        ("Committed (UTC)", committed(event.committed.as_ref())),
    ] {
        row(&mut body, &[esc(name), value]);
    }
    body.push_str("</tbody></table><h2>Recipients</h2>");
    if event.recipients.is_empty() {
        body.push_str("<p class=\"quiet\">No recipients recorded.</p>");
    } else {
        table(
            &mut body,
            &[
                "Recipient",
                "In snapshot",
                "Offers",
                "Offered (UTC)",
                "Receipted (UTC)",
                "To offer",
                "To receipt",
            ],
        );
        for r in &event.recipients {
            row(
                &mut body,
                &[
                    esc(&r.member),
                    if r.expected { "yes" } else { "no" }.into(),
                    r.offer_count.to_string(),
                    stamp(r.offered.as_ref()),
                    stamp(r.receipted.as_ref()),
                    interval(r.to_offer.as_ref()),
                    interval(r.to_receipt.as_ref()),
                ],
            );
        }
        body.push_str("</tbody></table>");
    }
    page(&format!("Event #{}", event.sequence), &body)
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
.text{white-space:pre-wrap;overflow-wrap:anywhere}\
textarea{width:100%;box-sizing:border-box;font:inherit}\
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
