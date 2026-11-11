//! Canonical requests (T-10, T-12): one row per question, with an owner who answers it and a
//! callback member who asked and is woken by the answer.
//!
//! The channel of an answer comes from the route that carried it, never from the caller: the
//! owner-authenticated route records `owner`, the owner member's own binding records `member`,
//! and any other member records `relayed`. A relay is a member passing on what the user said in
//! a chat. It is kept, and it moves the request to `relayed`, never to `answered`, so the user's
//! own answer is still accepted afterwards. Which relays are allowed is decided in one place,
//! [`relay_allowed`].
use crate::{
    bindings::{validate_on, Role, SessionBinding},
    facts::{self, ItemClass},
    store::Store,
};
use anyhow::{anyhow, bail, ensure, Result};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection};

const MAX_TEXT_CHARS: usize = 4000;
const MAX_REASON_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOwner {
    /// The user answers, on the owner route.
    User,
    /// One member answers, through its own binding.
    Member(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Open,
    /// A member passed on the user's answer. Not authenticated as the user's.
    Relayed,
    Answered,
    Withdrawn,
}

impl RequestState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Relayed => "relayed",
            Self::Answered => "answered",
            Self::Withdrawn => "withdrawn",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "open" => Self::Open,
            "relayed" => Self::Relayed,
            "answered" => Self::Answered,
            "withdrawn" => Self::Withdrawn,
            other => bail!("unknown request state {other}"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerChannel {
    Owner,
    Member,
    Relayed,
}

impl AnswerChannel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
            Self::Relayed => "relayed",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "owner" => Self::Owner,
            "member" => Self::Member,
            "relayed" => Self::Relayed,
            other => bail!("unknown answer channel {other}"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub request: String,
    pub callback: String,
    pub owner: RequestOwner,
    pub item: Option<String>,
    pub question: String,
    pub state: RequestState,
    pub withdraw_reason: Option<String>,
    pub created_wall_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub id: i64,
    pub channel: AnswerChannel,
    /// None for the owner route.
    pub actor: Option<String>,
    pub body: String,
    pub at_wall_ms: i64,
    /// Set once, by the callback, when it acts on the answer.
    pub applied_wall_ms: Option<i64>,
}

/// The relay policy, in one place so it can change without touching the routes. Proposed
/// default, pending the user's review: only a question for the user can be relayed, and only
/// until the user has answered it on the owner route. Any active member may relay, the asker
/// included.
fn relay_allowed(request: &Request) -> Result<()> {
    ensure!(
        request.owner == RequestOwner::User,
        "not authorized: only the owner answers a member's request"
    );
    ensure!(
        request.state != RequestState::Answered,
        "request already answered"
    );
    Ok(())
}

impl Store {
    /// A member asks the user or another member. The server picks the request's ID, so no member
    /// can collide with, or probe for, another's request; the asker's key covers retries. A
    /// question for a member needs the pair's active room, where the owner is told; it is refused
    /// rather than stored where nobody sees it. A question for the user needs no room.
    pub async fn open_request(
        &self,
        binding: &SessionBinding,
        owner: &RequestOwner,
        item: Option<&str>,
        question: &str,
        key: &str,
    ) -> Result<Request> {
        valid_id("request key", key)?;
        if let Some(item) = item {
            valid_id("work item id", item)?;
        }
        if let RequestOwner::Member(member) = owner {
            valid_id("owner", member)?;
        }
        let question = valid_text("question", question)?;
        let fingerprint = fingerprint(&serde_json::json!({
            "op": "open", "owner": owner, "item": item, "question": question,
        }));
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let status = validate_on(&mut tx, binding).await?;
        let context = &binding.context_id;
        let me = &binding.member_id;
        let prior = sqlx::query("SELECT request_id,fingerprint FROM candidate_requests WHERE context_id=? AND callback_member=? AND request_key=?")
            .bind(context).bind(me).bind(key).fetch_optional(&mut *tx).await?;
        if let Some(prior) = prior {
            ensure!(
                prior.try_get::<Vec<u8>, _>("fingerprint")? == fingerprint[..],
                "idempotency key conflicts with an earlier request"
            );
            let id: String = prior.try_get("request_id")?;
            let current = load_request(&mut tx, context, &id)
                .await?
                .ok_or_else(|| anyhow!("unknown request"))?;
            tx.commit().await?;
            return Ok(current);
        }
        if let Some(item) = item {
            // The work-item read rule: the item's owner or a repo lead. Anyone else gets the
            // same refusal as for a missing item.
            let owner: Option<String> = sqlx::query_scalar(
                "SELECT owner_member FROM candidate_work_items WHERE context_id=? AND item_id=?",
            )
            .bind(context)
            .bind(item)
            .fetch_optional(&mut *tx)
            .await?;
            ensure!(
                owner.is_some_and(|owner| owner == *me || status.role == Role::RepoLead),
                "unknown work item"
            );
        }
        let room = match owner {
            RequestOwner::User => None,
            RequestOwner::Member(member) => {
                ensure!(member != me, "a member does not ask itself");
                let active: i64 = sqlx::query_scalar("SELECT count(*) FROM candidate_members WHERE context_id=? AND member_id=? AND active=1")
                    .bind(context).bind(member).fetch_one(&mut *tx).await?;
                ensure!(active == 1, "owner unavailable");
                Some(
                    pair_room(&mut tx, context, me, member)
                        .await?
                        .ok_or_else(|| anyhow!("no active room with the owner"))?,
                )
            }
        };
        let now = facts::wall_ms();
        let request = &new_request_id()?;
        let created = Request {
            request: request.clone(),
            callback: me.clone(),
            owner: owner.clone(),
            item: item.map(Into::into),
            question,
            state: RequestState::Open,
            withdraw_reason: None,
            created_wall_ms: now,
        };
        let (owner_kind, owner_member) = owner_columns(owner);
        sqlx::query("INSERT INTO candidate_requests(context_id,request_id,callback_member,owner_kind,owner_member,item_id,question,state,request_key,fingerprint,created_wall_ms) VALUES (?,?,?,?,?,?,?,'open',?,?,?)")
            .bind(context).bind(request).bind(me).bind(owner_kind).bind(owner_member)
            .bind(item).bind(&created.question).bind(key).bind(&fingerprint[..]).bind(now)
            .execute(&mut *tx).await?;
        if let (Some(room), Some(owner)) = (room, owner_member) {
            let body = serde_json::json!({
                "request": request,
                "event": "opened",
                "callback": me,
                "item": item,
                "question": created.question,
            });
            self.emit(
                &mut tx, context, &room, me, request, "opened", None, &body, owner,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(created)
    }

    /// A member answers through its binding: as the owner of a member's request, or as a relay of
    /// the user's answer to a request for the user.
    pub async fn answer_request(
        &self,
        binding: &SessionBinding,
        request: &str,
        body: &str,
        key: &str,
    ) -> Result<Answer> {
        valid_id("request id", request)?;
        valid_id("request key", key)?;
        let body = valid_text("answer", body)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_on(&mut tx, binding).await?;
        let current = load_request(&mut tx, &binding.context_id, request)
            .await?
            .ok_or_else(|| anyhow!("unknown request"))?;
        let channel = match &current.owner {
            RequestOwner::Member(owner) if *owner == binding.member_id => AnswerChannel::Member,
            RequestOwner::Member(_) => {
                // The asker knows its request exists; anyone else can't tell it from a missing one.
                ensure!(current.callback == binding.member_id, "unknown request");
                bail!("not authorized: only the owner answers a member's request")
            }
            RequestOwner::User => AnswerChannel::Relayed,
        };
        let answer = self
            .record_answer(
                &mut tx,
                &binding.context_id,
                &current,
                channel,
                Some(&binding.member_id),
                &body,
                key,
            )
            .await?;
        tx.commit().await?;
        Ok(answer)
    }

    /// Trusted owner route: the user's own answer to a request for the user. Accepted after a
    /// relay; it is the authenticated answer the relay was not.
    pub async fn owner_answer_request(
        &self,
        context: &str,
        request: &str,
        body: &str,
        key: &str,
    ) -> Result<Answer> {
        valid_id("context", context)?;
        valid_id("request id", request)?;
        valid_id("request key", key)?;
        let body = valid_text("answer", body)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = load_request(&mut tx, context, request)
            .await?
            .ok_or_else(|| anyhow!("unknown request"))?;
        let answer = self
            .record_answer(
                &mut tx,
                context,
                &current,
                AnswerChannel::Owner,
                None,
                &body,
                key,
            )
            .await?;
        tx.commit().await?;
        Ok(answer)
    }

    /// Only the asker withdraws. The owner of a member's request is told, if their room is
    /// still active. Nothing is answered afterwards.
    pub async fn withdraw_request(
        &self,
        binding: &SessionBinding,
        request: &str,
        reason: &str,
    ) -> Result<Request> {
        valid_id("request id", request)?;
        let reason = valid_reason(reason)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_on(&mut tx, binding).await?;
        let context = &binding.context_id;
        let current = load_request(&mut tx, context, request)
            .await?
            .ok_or_else(|| anyhow!("unknown request"))?;
        ensure!(current.callback == binding.member_id, "unknown request");
        ensure!(
            matches!(current.state, RequestState::Open | RequestState::Relayed),
            "request is {}",
            current.state.as_str()
        );
        sqlx::query("UPDATE candidate_requests SET state='withdrawn', withdraw_reason=? WHERE context_id=? AND request_id=?")
            .bind(&reason).bind(context).bind(request).execute(&mut *tx).await?;
        if let RequestOwner::Member(owner) = &current.owner {
            if let Some(room) = pair_room(&mut tx, context, &current.callback, owner).await? {
                let body = serde_json::json!({
                    "request": request,
                    "event": "withdrawn",
                    "reason": reason,
                });
                self.emit(
                    &mut tx,
                    context,
                    &room,
                    &current.callback,
                    request,
                    "withdrawn",
                    None,
                    &body,
                    owner,
                )
                .await?;
            }
        }
        let withdrawn = load_request(&mut tx, context, request)
            .await?
            .ok_or_else(|| anyhow!("unknown request"))?;
        tx.commit().await?;
        Ok(withdrawn)
    }

    /// The asker records that it acted on an answer. The first time is kept.
    pub async fn mark_answer_applied(
        &self,
        binding: &SessionBinding,
        request: &str,
        answer: i64,
    ) -> Result<Answer> {
        valid_id("request id", request)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_on(&mut tx, binding).await?;
        let context = &binding.context_id;
        let current = load_request(&mut tx, context, request)
            .await?
            .ok_or_else(|| anyhow!("unknown request"))?;
        ensure!(current.callback == binding.member_id, "unknown request");
        sqlx::query("UPDATE candidate_request_answers SET applied_wall_ms=? WHERE context_id=? AND request_id=? AND answer_id=? AND applied_wall_ms IS NULL")
            .bind(facts::wall_ms()).bind(context).bind(request).bind(answer)
            .execute(&mut *tx).await?;
        let applied = load_answer(&mut tx, context, request, answer)
            .await?
            .ok_or_else(|| anyhow!("unknown answer"))?;
        tx.commit().await?;
        Ok(applied)
    }

    /// Trusted read.
    pub async fn request(&self, context: &str, request: &str) -> Result<Option<Request>> {
        let mut conn = self.pool.acquire().await?;
        load_request(&mut conn, context, request).await
    }

    /// Trusted read: every answer, in the order recorded.
    pub async fn request_answers(&self, context: &str, request: &str) -> Result<Vec<Answer>> {
        let rows = sqlx::query("SELECT answer_id,channel,actor_member,body,at_wall_ms,applied_wall_ms FROM candidate_request_answers WHERE context_id=? AND request_id=? ORDER BY answer_id")
            .bind(context).bind(request).fetch_all(&self.pool).await?;
        rows.iter().map(answer_from).collect()
    }

    /// Trusted read: every request not yet answered or withdrawn, whoever owns it, by ID. A
    /// relayed request is included, since the user has not answered it; `waits_now` leaves it
    /// out, since the asker has an answer to act on.
    pub async fn open_requests(&self, context: &str) -> Result<Vec<Request>> {
        let ids: Vec<String> = sqlx::query_scalar("SELECT request_id FROM candidate_requests WHERE context_id=? AND state IN ('open','relayed') ORDER BY request_id")
            .bind(context).fetch_all(&self.pool).await?;
        let mut conn = self.pool.acquire().await?;
        let mut requests = Vec::with_capacity(ids.len());
        for id in ids {
            requests.push(
                load_request(&mut conn, context, &id)
                    .await?
                    .ok_or_else(|| anyhow!("unknown request"))?,
            );
        }
        Ok(requests)
    }

    /// Record one answer and wake the asker. A retried key returns the first answer before any
    /// state check, so a retry after the request moved on still succeeds.
    #[allow(clippy::too_many_arguments)]
    async fn record_answer(
        &self,
        conn: &mut SqliteConnection,
        context: &str,
        request: &Request,
        channel: AnswerChannel,
        actor: Option<&str>,
        body: &str,
        key: &str,
    ) -> Result<Answer> {
        let fingerprint = fingerprint(&serde_json::json!({
            "op": "answer", "request": request.request, "body": body,
        }));
        let prior = sqlx::query("SELECT answer_id,request_id,fingerprint FROM candidate_request_answers WHERE context_id=? AND channel=? AND coalesce(actor_member,'')=? AND request_key=?")
            .bind(context).bind(channel.as_str()).bind(actor.unwrap_or("")).bind(key)
            .fetch_optional(&mut *conn).await?;
        if let Some(prior) = prior {
            ensure!(
                prior.try_get::<Vec<u8>, _>("fingerprint")? == fingerprint[..],
                "idempotency key conflicts with an earlier request"
            );
            let id: i64 = prior.try_get("answer_id")?;
            let request_id: String = prior.try_get("request_id")?;
            return load_answer(conn, context, &request_id, id)
                .await?
                .ok_or_else(|| anyhow!("unknown answer"));
        }
        ensure!(
            request.state != RequestState::Withdrawn,
            "request withdrawn: {}",
            request.withdraw_reason.as_deref().unwrap_or("")
        );
        let next = match channel {
            AnswerChannel::Owner => {
                ensure!(
                    request.owner == RequestOwner::User,
                    "not authorized: a member's request is answered by that member"
                );
                ensure!(
                    request.state != RequestState::Answered,
                    "request already answered"
                );
                RequestState::Answered
            }
            AnswerChannel::Member => {
                ensure!(
                    request.state != RequestState::Answered,
                    "request already answered"
                );
                RequestState::Answered
            }
            AnswerChannel::Relayed => {
                relay_allowed(request)?;
                RequestState::Relayed
            }
        };
        let now = facts::wall_ms();
        let id: i64 = sqlx::query_scalar("INSERT INTO candidate_request_answers(context_id,request_id,channel,actor_member,body,request_key,fingerprint,at_wall_ms) VALUES (?,?,?,?,?,?,?,?) RETURNING answer_id")
            .bind(context).bind(&request.request).bind(channel.as_str()).bind(actor)
            .bind(body).bind(key).bind(&fingerprint[..]).bind(now)
            .fetch_one(&mut *conn).await?;
        sqlx::query("UPDATE candidate_requests SET state=? WHERE context_id=? AND request_id=?")
            .bind(next.as_str())
            .bind(context)
            .bind(&request.request)
            .execute(&mut *conn)
            .await?;
        // The asker is woken unless it is the one answering, which only a relay can be.
        if actor != Some(request.callback.as_str()) {
            let with = match &request.owner {
                RequestOwner::Member(owner) => Some(owner.as_str()),
                RequestOwner::User => None,
            };
            if let Some(room) = callback_room(conn, context, &request.callback, with).await? {
                let event = serde_json::json!({
                    "request": request.request,
                    "event": "answered",
                    "channel": channel,
                    "answer": id,
                    "body": body,
                });
                self.emit(
                    conn,
                    context,
                    &room,
                    &request.callback,
                    &request.request,
                    "answered",
                    Some(id),
                    &event,
                    &request.callback,
                )
                .await?;
            }
        }
        load_answer(conn, context, &request.request, id)
            .await?
            .ok_or_else(|| anyhow!("unknown answer"))
    }

    /// One request event to one recipient, with its delivery facts, in the caller's transaction.
    #[allow(clippy::too_many_arguments)]
    async fn emit(
        &self,
        conn: &mut SqliteConnection,
        context: &str,
        room: &str,
        sender: &str,
        request: &str,
        event: &str,
        answer: Option<i64>,
        body: &serde_json::Value,
        recipient: &str,
    ) -> Result<()> {
        // The stored sender is the asker, a member of every room an event goes to, for the
        // item's foreign keys; readers see 'cbc'.
        let sequence: i64 = sqlx::query_scalar("INSERT INTO candidate_items(context_id,room_id,sender,kind,body) VALUES (?,?,?,'message',?) RETURNING sequence")
            .bind(context).bind(room).bind(sender).bind(body.to_string())
            .fetch_one(&mut *conn).await?;
        sqlx::query("INSERT INTO candidate_request_events(sequence,context_id,request_id,event,answer_id) VALUES (?,?,?,?,?)")
            .bind(sequence).bind(context).bind(request).bind(event).bind(answer)
            .execute(&mut *conn).await?;
        sqlx::query(
            "INSERT INTO candidate_request_event_recipients(sequence,member_id) VALUES (?,?)",
        )
        .bind(sequence)
        .bind(recipient)
        .execute(&mut *conn)
        .await?;
        facts::record_commit(conn, &self.clock, sequence, ItemClass::RequestEvent).await
    }
}

/// Retirement ends the member's open requests, as asker and as owner, in the retirement's
/// transaction. No events: the member's rooms are closed in the same transaction and say why.
pub(crate) async fn withdraw_for_retirement(
    conn: &mut SqliteConnection,
    context: &str,
    member: &str,
    note: &str,
) -> Result<()> {
    sqlx::query("UPDATE candidate_requests SET state='withdrawn', withdraw_reason=? WHERE context_id=? AND state IN ('open','relayed') AND (callback_member=? OR owner_member=?)")
        .bind(note).bind(context).bind(member).bind(member).execute(conn).await?;
    Ok(())
}

/// The pair's active, open room, of either kind.
async fn pair_room(
    conn: &mut SqliteConnection,
    context: &str,
    a: &str,
    b: &str,
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT c.room_id FROM candidate_connections c JOIN candidate_rooms r ON r.context_id=c.context_id AND r.room_id=c.room_id AND r.closed=0 WHERE c.context_id=? AND min(c.first_member,c.second_member)=min(?,?) AND max(c.first_member,c.second_member)=max(?,?) AND c.active=1")
        .bind(context).bind(a).bind(b).bind(a).bind(b).fetch_optional(conn).await?)
}

/// Where the asker hears an answer: the room shared with the owner member if it is still
/// active, otherwise the asker's first active room, report rooms first. None leaves the answer
/// recorded and undelivered.
async fn callback_room(
    conn: &mut SqliteConnection,
    context: &str,
    callback: &str,
    with: Option<&str>,
) -> Result<Option<String>> {
    if let Some(with) = with {
        if let Some(room) = pair_room(conn, context, callback, with).await? {
            return Ok(Some(room));
        }
    }
    Ok(sqlx::query_scalar("SELECT c.room_id FROM candidate_connections c JOIN candidate_rooms r ON r.context_id=c.context_id AND r.room_id=c.room_id AND r.closed=0 JOIN candidate_room_members m ON m.context_id=c.context_id AND m.room_id=c.room_id AND m.member_id=? AND m.active=1 WHERE c.context_id=? AND c.active=1 AND (c.first_member=? OR c.second_member=?) ORDER BY CASE c.kind WHEN 'report' THEN 0 ELSE 1 END, c.room_id LIMIT 1")
        .bind(callback).bind(context).bind(callback).bind(callback).fetch_optional(conn).await?)
}

fn owner_columns(owner: &RequestOwner) -> (&'static str, Option<&str>) {
    match owner {
        RequestOwner::User => ("user", None),
        RequestOwner::Member(member) => ("member", Some(member.as_str())),
    }
}

async fn load_request(
    conn: &mut SqliteConnection,
    context: &str,
    request: &str,
) -> Result<Option<Request>> {
    let row = sqlx::query("SELECT callback_member,owner_kind,owner_member,item_id,question,state,withdraw_reason,created_wall_ms FROM candidate_requests WHERE context_id=? AND request_id=?")
        .bind(context).bind(request).fetch_optional(conn).await?;
    row.map(|row| {
        let owner = match row.try_get::<String, _>("owner_kind")?.as_str() {
            "user" => RequestOwner::User,
            "member" => RequestOwner::Member(row.try_get("owner_member")?),
            other => bail!("unknown request owner {other}"),
        };
        Ok(Request {
            request: request.into(),
            callback: row.try_get("callback_member")?,
            owner,
            item: row.try_get("item_id")?,
            question: row.try_get("question")?,
            state: RequestState::parse(row.try_get("state")?)?,
            withdraw_reason: row.try_get("withdraw_reason")?,
            created_wall_ms: row.try_get("created_wall_ms")?,
        })
    })
    .transpose()
}

async fn load_answer(
    conn: &mut SqliteConnection,
    context: &str,
    request: &str,
    answer: i64,
) -> Result<Option<Answer>> {
    let row = sqlx::query("SELECT answer_id,channel,actor_member,body,at_wall_ms,applied_wall_ms FROM candidate_request_answers WHERE context_id=? AND request_id=? AND answer_id=?")
        .bind(context).bind(request).bind(answer).fetch_optional(conn).await?;
    row.as_ref().map(answer_from).transpose()
}

fn answer_from(row: &sqlx::sqlite::SqliteRow) -> Result<Answer> {
    Ok(Answer {
        id: row.try_get("answer_id")?,
        channel: AnswerChannel::parse(row.try_get("channel")?)?,
        actor: row.try_get("actor_member")?,
        body: row.try_get("body")?,
        at_wall_ms: row.try_get("at_wall_ms")?,
        applied_wall_ms: row.try_get("applied_wall_ms")?,
    })
}

/// 128 random bits: unguessable, so an ID is only known to whoever was told it.
fn new_request_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    OsRng.try_fill_bytes(&mut bytes)?;
    Ok(format!("req-{}", hex(&bytes)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A stable digest of a request, as for work operations, under its own domain.
fn fingerprint(request: &serde_json::Value) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"cbc-request-v1\0");
    hasher.update(request.to_string().as_bytes());
    hasher.finalize().into()
}

fn valid_id(field: &str, value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control),
        "invalid {field}"
    );
    Ok(())
}

fn valid_text(field: &str, value: &str) -> Result<String> {
    let value = value.trim();
    ensure!(!value.is_empty(), "invalid {field}: empty");
    ensure!(
        value.chars().count() <= MAX_TEXT_CHARS,
        "invalid {field}: longer than {MAX_TEXT_CHARS} characters"
    );
    ensure!(
        !value.chars().any(|c| c.is_control() && c != '\n'),
        "invalid {field}: control characters"
    );
    Ok(value.into())
}

fn valid_reason(reason: &str) -> Result<String> {
    let reason = reason.trim();
    ensure!(!reason.is_empty(), "invalid reason: empty");
    ensure!(
        reason.chars().count() <= MAX_REASON_CHARS,
        "invalid reason: longer than {MAX_REASON_CHARS} characters"
    );
    ensure!(
        !reason.chars().any(char::is_control),
        "invalid reason: must be one line"
    );
    Ok(reason.into())
}
