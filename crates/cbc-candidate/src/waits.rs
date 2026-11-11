//! The T-27 waits mapping: which record carries each wait cause of the trial protocol (§6 of
//! `docs/design/working-trial-protocol.md`), and the causes derivable from the records today.
//!
//! A cause the records can't carry yet says what is missing. A wait whose cause can't be
//! derived is reported with no cause, never a guessed one.
use crate::{bindings::Role, store::Store};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitCause {
    User,
    HumanReview,
    Primarch,
    Resource,
    Ci,
    ChapterMaster,
    Delivery,
    ReadyIdle,
}

/// How much of a cause the records carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Derivation {
    /// Fully derivable from records that exist.
    Known,
    /// Derivable in part; `missing` names the record still to come.
    Partial { missing: &'static str },
    /// Not derivable yet; `needs` names the record it waits on.
    Unknown { needs: &'static str },
}

impl WaitCause {
    /// In the protocol table's order.
    pub const ALL: [Self; 8] = [
        Self::User,
        Self::HumanReview,
        Self::Primarch,
        Self::Resource,
        Self::Ci,
        Self::ChapterMaster,
        Self::Delivery,
        Self::ReadyIdle,
    ];

    /// The protocol's own wording.
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "waiting on the user",
            Self::HumanReview => "waiting on human review, by policy",
            Self::Primarch => "waiting on the Primarch",
            Self::Resource => "waiting on a resource",
            Self::Ci => "waiting on CI",
            Self::ChapterMaster => "waiting on another Chapter Master",
            Self::Delivery => "waiting on delivery",
            Self::ReadyIdle => "ready but idle",
        }
    }

    pub fn derivation(self) -> Derivation {
        match self {
            Self::User | Self::ChapterMaster | Self::Delivery => Derivation::Known,
            Self::HumanReview => Derivation::Partial {
                missing: "a PR observation saying the PR is ready (the HITL flag exists)",
            },
            Self::Primarch => Derivation::Partial {
                missing: "holds (#114); open requests to the Primarch are derived",
            },
            Self::Resource => Derivation::Unknown {
                needs: "holds on named resources (#114)",
            },
            Self::Ci => Derivation::Unknown {
                needs: "PR observations with check state on the current head",
            },
            Self::ReadyIdle => Derivation::Partial {
                missing: "holds (#114): receipt to first activity is derived, not whether anything else blocked the member",
            },
        }
    }
}

/// One open obligation of a member, now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wait {
    /// None when the records don't say why: reported as `unknown`.
    pub cause: Option<WaitCause>,
    pub request: Option<String>,
    pub sequence: Option<i64>,
    /// None for an item committed before delivery facts existed.
    pub since_wall_ms: Option<i64>,
}

/// Whose request it is decides the cause: a repo lead is the Primarch, a feature lead another
/// Chapter Master. Any other role has no §6 cause.
fn owner_cause(role: Option<Role>) -> Option<WaitCause> {
    match role? {
        Role::RepoLead => Some(WaitCause::Primarch),
        Role::FeatureLead => Some(WaitCause::ChapterMaster),
        _ => None,
    }
}

impl Store {
    /// Trusted read: the member's waits derivable now. Its open requests, by owner, and its
    /// unreceipted items. A relayed request is not a wait: the asker has an answer to act on.
    pub async fn waits_now(&self, context: &str, member: &str) -> Result<Vec<Wait>> {
        let mut waits = Vec::new();
        let rows = sqlx::query("SELECT q.request_id,q.owner_kind,m.role,q.created_wall_ms FROM candidate_requests q LEFT JOIN candidate_members m ON m.context_id=q.context_id AND m.member_id=q.owner_member WHERE q.context_id=? AND q.callback_member=? AND q.state='open' ORDER BY q.request_id")
            .bind(context).bind(member).fetch_all(&self.pool).await?;
        for row in rows {
            let cause = match row.try_get::<String, _>("owner_kind")?.as_str() {
                "user" => Some(WaitCause::User),
                _ => owner_cause(
                    row.try_get::<Option<String>, _>("role")?
                        .as_deref()
                        .map(Role::parse)
                        .transpose()?,
                ),
            };
            waits.push(Wait {
                cause,
                request: Some(row.try_get("request_id")?),
                sequence: None,
                since_wall_ms: Some(row.try_get("created_wall_ms")?),
            });
        }
        let rows = sqlx::query("SELECT DISTINCT u.sequence,f.committed_wall_ms FROM candidate_unread_items u LEFT JOIN candidate_item_facts f ON f.sequence=u.sequence WHERE u.context_id=? AND u.recipient=? ORDER BY u.sequence")
            .bind(context).bind(member).fetch_all(&self.pool).await?;
        for row in rows {
            waits.push(Wait {
                cause: Some(WaitCause::Delivery),
                request: None,
                sequence: Some(row.try_get("sequence")?),
                since_wall_ms: row.try_get("committed_wall_ms")?,
            });
        }
        Ok(waits)
    }
}
