//! Work-item routes (#113 R2b). The connector only wires these in; the rules live in
//! `crate::work`.
use super::{LocalClient, LocalOwnerClient, Operation, OwnerOperation, Reply};
use crate::{
    bindings::SessionBinding,
    store::Store,
    work::{WorkItem, WorkOutcome, WorkPatch, WorkRevision},
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum WorkOperation {
    Register {
        item: String,
        summary: String,
        key: String,
    },
    Update {
        item: String,
        expected_revision: i64,
        key: String,
        patch: WorkPatch,
    },
    SetHitl {
        item: String,
        on: bool,
        key: String,
    },
    Get {
        item: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OwnerWorkOperation {
    SetHitl {
        context: String,
        item: String,
        on: bool,
        key: String,
    },
    Get {
        context: String,
        item: String,
    },
    Revisions {
        context: String,
        item: String,
        after: i64,
        limit: u32,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum WorkReply {
    /// A registration, which may have committed a work event.
    Registered {
        item: WorkItem,
    },
    Outcome {
        outcome: WorkOutcome,
    },
    Item {
        item: WorkItem,
    },
}

impl WorkReply {
    /// Whether a parked watcher may have something new to read.
    pub(super) fn committed(&self) -> bool {
        matches!(
            self,
            WorkReply::Registered { .. }
                | WorkReply::Outcome {
                    outcome: WorkOutcome::Applied { .. }
                }
        )
    }
}

pub(super) async fn dispatch(
    store: &Store,
    binding: &SessionBinding,
    operation: WorkOperation,
) -> Result<WorkReply> {
    Ok(match operation {
        WorkOperation::Register { item, summary, key } => WorkReply::Registered {
            item: store
                .register_work_item(binding, &item, &summary, &key)
                .await?,
        },
        WorkOperation::Update {
            item,
            expected_revision,
            key,
            patch,
        } => WorkReply::Outcome {
            outcome: store
                .update_work_item(binding, &item, expected_revision, &key, &patch)
                .await?,
        },
        WorkOperation::SetHitl { item, on, key } => WorkReply::Outcome {
            outcome: store.set_work_hitl(binding, &item, on, &key).await?,
        },
        WorkOperation::Get { item } => WorkReply::Item {
            item: store.read_work_item(binding, &item).await?,
        },
    })
}

pub(super) async fn owner_dispatch(
    store: &Store,
    operation: OwnerWorkOperation,
) -> Result<serde_json::Value> {
    Ok(match operation {
        OwnerWorkOperation::SetHitl {
            context,
            item,
            on,
            key,
        } => serde_json::json!({
            "outcome": store.owner_set_work_hitl(&context, &item, on, &key).await?,
        }),
        // The item alone: its history comes in pages, so the reply never outgrows the limit.
        OwnerWorkOperation::Get { context, item } => serde_json::json!({
            "item": store
                .work_item(&context, &item)
                .await?
                .context("unknown work item")?,
        }),
        OwnerWorkOperation::Revisions {
            context,
            item,
            after,
            limit,
        } => serde_json::json!({
            "revisions": store.work_revision_page(&context, &item, after, limit).await?,
        }),
    })
}

impl LocalClient<'_> {
    async fn work(&self, operation: WorkOperation) -> Result<WorkReply> {
        match self.call(Operation::Work { work: operation }).await? {
            Reply::Work { result } => Ok(result),
            _ => bail!("unexpected candidate response"),
        }
    }
    pub async fn register_work_item(
        &self,
        item: &str,
        summary: &str,
        key: &str,
    ) -> Result<WorkItem> {
        match self
            .work(WorkOperation::Register {
                item: item.into(),
                summary: summary.into(),
                key: key.into(),
            })
            .await?
        {
            WorkReply::Registered { item } => Ok(item),
            _ => bail!("unexpected candidate response"),
        }
    }
    pub async fn update_work_item(
        &self,
        item: &str,
        expected_revision: i64,
        key: &str,
        patch: &WorkPatch,
    ) -> Result<WorkOutcome> {
        match self
            .work(WorkOperation::Update {
                item: item.into(),
                expected_revision,
                key: key.into(),
                patch: patch.clone(),
            })
            .await?
        {
            WorkReply::Outcome { outcome } => Ok(outcome),
            _ => bail!("unexpected candidate response"),
        }
    }
    pub async fn set_work_hitl(&self, item: &str, on: bool, key: &str) -> Result<WorkOutcome> {
        match self
            .work(WorkOperation::SetHitl {
                item: item.into(),
                on,
                key: key.into(),
            })
            .await?
        {
            WorkReply::Outcome { outcome } => Ok(outcome),
            _ => bail!("unexpected candidate response"),
        }
    }
    pub async fn work_item(&self, item: &str) -> Result<WorkItem> {
        match self.work(WorkOperation::Get { item: item.into() }).await? {
            WorkReply::Item { item } => Ok(item),
            _ => bail!("unexpected candidate response"),
        }
    }
}

impl LocalOwnerClient<'_> {
    /// T-07: the user's own HITL control, the only route that turns it off.
    pub async fn set_work_hitl(
        &self,
        context: &str,
        item: &str,
        on: bool,
        key: &str,
    ) -> Result<WorkOutcome> {
        let value = self
            .call(OwnerOperation::Work {
                work: OwnerWorkOperation::SetHitl {
                    context: context.into(),
                    item: item.into(),
                    on,
                    key: key.into(),
                },
            })
            .await?;
        serde_json::from_value(value["outcome"].clone()).context("invalid candidate work result")
    }
    pub async fn work_item(&self, context: &str, item: &str) -> Result<WorkItem> {
        let value = self
            .call(OwnerOperation::Work {
                work: OwnerWorkOperation::Get {
                    context: context.into(),
                    item: item.into(),
                },
            })
            .await?;
        serde_json::from_value(value["item"].clone()).context("invalid candidate work item")
    }
    /// Revisions after `after`, oldest first; the core caps a page at
    /// [`crate::work::MAX_REVISION_PAGE`].
    pub async fn work_revisions(
        &self,
        context: &str,
        item: &str,
        after: i64,
        limit: u32,
    ) -> Result<Vec<WorkRevision>> {
        let value = self
            .call(OwnerOperation::Work {
                work: OwnerWorkOperation::Revisions {
                    context: context.into(),
                    item: item.into(),
                    after,
                    limit,
                },
            })
            .await?;
        serde_json::from_value(value["revisions"].clone())
            .context("invalid candidate work revisions")
    }
}
