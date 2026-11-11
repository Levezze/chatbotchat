//! Request routes (#113 R3b). The connector only wires these in; the rules, including which
//! channel an answer takes, live in `crate::requests`.
use super::{LocalClient, LocalOwnerClient, Operation, OwnerOperation, Reply};
use crate::{
    bindings::SessionBinding,
    requests::{Answer, Request, RequestOwner},
    store::Store,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RequestOperation {
    Open {
        owner: RequestOwner,
        item: Option<String>,
        question: String,
        key: String,
    },
    Answer {
        request: String,
        body: String,
        key: String,
    },
    Withdraw {
        request: String,
        reason: String,
    },
    Applied {
        request: String,
        answer: i64,
    },
    Get {
        request: String,
    },
    Answers {
        request: String,
        after: i64,
        limit: u32,
    },
    Pending {
        after: String,
        limit: u32,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OwnerRequestOperation {
    Answer {
        context: String,
        request: String,
        body: String,
        key: String,
    },
    Get {
        context: String,
        request: String,
    },
    Answers {
        context: String,
        request: String,
        after: i64,
        limit: u32,
    },
    Pending {
        context: String,
        after: String,
        limit: u32,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RequestReply {
    Opened { request: Request },
    Answered { answer: Answer },
    Withdrawn { request: Request },
    Applied { answer: Answer },
    Request { request: Request },
    Answers { answers: Vec<Answer> },
    Requests { requests: Vec<Request> },
}

impl RequestReply {
    /// Whether the operation may have committed a request event, so a parked watcher should
    /// look again. A replayed open says yes too; the watcher finds nothing and parks again.
    pub(super) fn committed(&self) -> bool {
        matches!(
            self,
            RequestReply::Opened { .. }
                | RequestReply::Answered { .. }
                | RequestReply::Withdrawn { .. }
        )
    }
}

pub(super) async fn dispatch(
    store: &Store,
    binding: &SessionBinding,
    operation: RequestOperation,
) -> Result<RequestReply> {
    Ok(match operation {
        RequestOperation::Open {
            owner,
            item,
            question,
            key,
        } => RequestReply::Opened {
            request: store
                .open_request(binding, &owner, item.as_deref(), &question, &key)
                .await?,
        },
        RequestOperation::Answer { request, body, key } => RequestReply::Answered {
            answer: store.answer_request(binding, &request, &body, &key).await?,
        },
        RequestOperation::Withdraw { request, reason } => RequestReply::Withdrawn {
            request: store.withdraw_request(binding, &request, &reason).await?,
        },
        RequestOperation::Applied { request, answer } => RequestReply::Applied {
            answer: store.mark_answer_applied(binding, &request, answer).await?,
        },
        RequestOperation::Get { request } => RequestReply::Request {
            request: store.read_request(binding, &request).await?,
        },
        RequestOperation::Answers {
            request,
            after,
            limit,
        } => RequestReply::Answers {
            answers: store
                .read_request_answers(binding, &request, after, limit)
                .await?,
        },
        RequestOperation::Pending { after, limit } => RequestReply::Requests {
            requests: store.pending_requests(binding, &after, limit).await?,
        },
    })
}

pub(super) async fn owner_dispatch(
    store: &Store,
    operation: OwnerRequestOperation,
) -> Result<serde_json::Value> {
    Ok(match operation {
        OwnerRequestOperation::Answer {
            context,
            request,
            body,
            key,
        } => serde_json::json!({
            "answer": store.owner_answer_request(&context, &request, &body, &key).await?,
        }),
        OwnerRequestOperation::Get { context, request } => serde_json::json!({
            "request": store
                .request(&context, &request)
                .await?
                .context("unknown request")?,
        }),
        OwnerRequestOperation::Answers {
            context,
            request,
            after,
            limit,
        } => serde_json::json!({
            "answers": store
                .owner_request_answers(&context, &request, after, limit)
                .await?,
        }),
        OwnerRequestOperation::Pending {
            context,
            after,
            limit,
        } => serde_json::json!({
            "requests": store.owner_pending_requests(&context, &after, limit).await?,
        }),
    })
}

impl LocalClient<'_> {
    async fn requests(&self, operation: RequestOperation) -> Result<RequestReply> {
        match self.call(Operation::Request { request: operation }).await? {
            Reply::Request { result } => Ok(result),
            _ => bail!("unexpected candidate response"),
        }
    }
    /// Ask the user (`RequestOwner::User`) or one member. The server picks the request ID; a
    /// retried `key` returns the first request.
    pub async fn open_request(
        &self,
        owner: &RequestOwner,
        item: Option<&str>,
        question: &str,
        key: &str,
    ) -> Result<Request> {
        match self
            .requests(RequestOperation::Open {
                owner: owner.clone(),
                item: item.map(Into::into),
                question: question.into(),
                key: key.into(),
            })
            .await?
        {
            RequestReply::Opened { request } => Ok(request),
            _ => bail!("unexpected candidate response"),
        }
    }
    /// Answer as the owner of a member's request, or relay the user's answer to a request for
    /// the user. The server decides which from who is asking.
    pub async fn answer_request(&self, request: &str, body: &str, key: &str) -> Result<Answer> {
        match self
            .requests(RequestOperation::Answer {
                request: request.into(),
                body: body.into(),
                key: key.into(),
            })
            .await?
        {
            RequestReply::Answered { answer } => Ok(answer),
            _ => bail!("unexpected candidate response"),
        }
    }
    pub async fn withdraw_request(&self, request: &str, reason: &str) -> Result<Request> {
        match self
            .requests(RequestOperation::Withdraw {
                request: request.into(),
                reason: reason.into(),
            })
            .await?
        {
            RequestReply::Withdrawn { request } => Ok(request),
            _ => bail!("unexpected candidate response"),
        }
    }
    pub async fn mark_answer_applied(&self, request: &str, answer: i64) -> Result<Answer> {
        match self
            .requests(RequestOperation::Applied {
                request: request.into(),
                answer,
            })
            .await?
        {
            RequestReply::Applied { answer } => Ok(answer),
            _ => bail!("unexpected candidate response"),
        }
    }
    pub async fn request(&self, request: &str) -> Result<Request> {
        match self
            .requests(RequestOperation::Get {
                request: request.into(),
            })
            .await?
        {
            RequestReply::Request { request } => Ok(request),
            _ => bail!("unexpected candidate response"),
        }
    }
    /// Answers after answer `after`, oldest first; the core caps a page at
    /// [`crate::requests::MAX_REQUEST_PAGE`].
    pub async fn request_answers(
        &self,
        request: &str,
        after: i64,
        limit: u32,
    ) -> Result<Vec<Answer>> {
        match self
            .requests(RequestOperation::Answers {
                request: request.into(),
                after,
                limit,
            })
            .await?
        {
            RequestReply::Answers { answers } => Ok(answers),
            _ => bail!("unexpected candidate response"),
        }
    }
    /// The member's own waiting requests after ID `after` ("" for the first page).
    pub async fn pending_requests(&self, after: &str, limit: u32) -> Result<Vec<Request>> {
        match self
            .requests(RequestOperation::Pending {
                after: after.into(),
                limit,
            })
            .await?
        {
            RequestReply::Requests { requests } => Ok(requests),
            _ => bail!("unexpected candidate response"),
        }
    }
}

impl LocalOwnerClient<'_> {
    async fn requests(&self, operation: OwnerRequestOperation) -> Result<serde_json::Value> {
        self.call(OwnerOperation::Request { request: operation })
            .await
    }
    /// The user's own answer to a request for the user: the authenticated one.
    pub async fn answer_request(
        &self,
        context: &str,
        request: &str,
        body: &str,
        key: &str,
    ) -> Result<Answer> {
        let value = self
            .requests(OwnerRequestOperation::Answer {
                context: context.into(),
                request: request.into(),
                body: body.into(),
                key: key.into(),
            })
            .await?;
        serde_json::from_value(value["answer"].clone()).context("invalid candidate answer")
    }
    pub async fn request(&self, context: &str, request: &str) -> Result<Request> {
        let value = self
            .requests(OwnerRequestOperation::Get {
                context: context.into(),
                request: request.into(),
            })
            .await?;
        serde_json::from_value(value["request"].clone()).context("invalid candidate request")
    }
    pub async fn request_answers(
        &self,
        context: &str,
        request: &str,
        after: i64,
        limit: u32,
    ) -> Result<Vec<Answer>> {
        let value = self
            .requests(OwnerRequestOperation::Answers {
                context: context.into(),
                request: request.into(),
                after,
                limit,
            })
            .await?;
        serde_json::from_value(value["answers"].clone()).context("invalid candidate answers")
    }
    /// Every waiting request in the context after ID `after` ("" for the first page).
    pub async fn pending_requests(
        &self,
        context: &str,
        after: &str,
        limit: u32,
    ) -> Result<Vec<Request>> {
        let value = self
            .requests(OwnerRequestOperation::Pending {
                context: context.into(),
                after: after.into(),
                limit,
            })
            .await?;
        serde_json::from_value(value["requests"].clone()).context("invalid candidate requests")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unknown fields are refused on both routes, so a caller can't pass a channel or actor.
    #[test]
    fn request_operations_refuse_unknown_fields() {
        assert!(serde_json::from_str::<RequestOperation>(
            r#"{"op":"answer","request":"req-1","body":"b","key":"k","channel":"owner"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<RequestOperation>(
            r#"{"op":"answer","request":"req-1","body":"b","key":"k"}"#
        )
        .is_ok());
        assert!(serde_json::from_str::<OwnerRequestOperation>(
            r#"{"op":"get","context":"c","request":"req-1","actor":"lead"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<OwnerRequestOperation>(
            r#"{"op":"get","context":"c","request":"req-1"}"#
        )
        .is_ok());
    }
}
