//! The work-item fixture shared by `work_items.rs` and `work_followups.rs`: a feature lead
//! (`worker`) reporting to a repo lead (`lead`) in an open report room, and a second feature
//! lead (`other`) with no room.
#![allow(dead_code)]
use cbc_candidate::{
    bindings::{Role, SessionBinding},
    enrollment::ConnectionKind,
    store::Store,
    work::WorkPatch,
    Sandbox,
};
use serde_json::Value;
use tempfile::{tempdir, TempDir};

pub struct Case {
    pub _temp: TempDir,
    pub _sandbox: Sandbox,
    pub store: Store,
    pub worker: SessionBinding,
    pub lead: SessionBinding,
    pub other: SessionBinding,
    pub report_room: String,
}

pub async fn fixture() -> Case {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8989,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("project").await.unwrap();
    for (member, role) in [
        ("worker", Role::FeatureLead),
        ("lead", Role::RepoLead),
        ("other", Role::FeatureLead),
    ] {
        store
            .register_member("project", member, role)
            .await
            .unwrap();
    }
    let bind = |m: &'static str| {
        let store = &store;
        async move {
            store
                .bind_member("project", m, "device", &format!("{m}-session"))
                .await
                .unwrap()
        }
    };
    let worker = bind("worker").await;
    let lead = bind("lead").await;
    let other = bind("other").await;
    store
        .authorize_connection(
            "project",
            "worker",
            "lead",
            ConnectionKind::Report,
            "feature",
        )
        .await
        .unwrap();
    let report_room = store
        .connect_members(&worker, "lead", ConnectionKind::Report, "feature", "r1", "")
        .await
        .unwrap()
        .room;
    let case = Case {
        _temp: temp,
        _sandbox: sandbox,
        store,
        worker,
        lead,
        other,
        report_room,
    };
    // Start every test from an empty inbox: the introduction is not a work event.
    drain(&case.store, &case.worker).await;
    drain(&case.store, &case.lead).await;
    case
}

/// Every work event a member receives, as (room, parsed body). Receipts everything it reads.
pub async fn drain(store: &Store, who: &SessionBinding) -> Vec<(String, Value)> {
    let mut events = Vec::new();
    while let Some(batch) = store.check(who, 32).await.unwrap() {
        for item in &batch.items {
            if item.kind == "work_event" {
                assert_eq!(item.sender, "cbc", "a work event is server information");
                events.push((item.room.clone(), serde_json::from_str(&item.body).unwrap()));
            }
        }
        store
            .receipt(who, batch.reference_for_adapter())
            .await
            .unwrap();
    }
    events
}

pub fn patch() -> WorkPatch {
    WorkPatch::default()
}
