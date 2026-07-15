//! Data types for the notification inbox: the `notifications` row, the insert
//! shape, and paged-list query params. Generic + domain-agnostic — kind-specific
//! data rides the `payload` JSONB column (the SDK schema knows no domain).

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A row of `notifications`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, sqlx::FromRow)]
pub struct Notification {
    pub id: Uuid,
    pub user_id: Uuid,
    /// The contributing module's notification kind (e.g. `study_share_invite`);
    /// the frontend dispatches its renderer on this.
    pub kind: String,
    pub title: String,
    pub body: String,
    /// TRUE => client may toast on arrival; FALSE => durable inbox row only.
    pub interrupt: bool,
    /// Kind-specific structured data the FE renderer reads (e.g.
    /// `{study_id, share_id}`). Defaults to `{}`.
    pub payload: serde_json::Value,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Notification {
    pub fn is_unread(&self) -> bool {
        self.read_at.is_none()
    }
}

/// Insert shape for a new notification (the `create_and_emit` seam input).
#[derive(Debug, Clone)]
pub struct NewNotification {
    pub user_id: Uuid,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub interrupt: bool,
    pub payload: serde_json::Value,
}

impl NewNotification {
    /// A minimal notification for `user_id` with a kind + title. Interrupts
    /// (toasts) by default; call `.silent()` for a durable-only row.
    pub fn new(user_id: Uuid, kind: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            user_id,
            kind: kind.into(),
            title: title.into(),
            body: String::new(),
            interrupt: true,
            payload: serde_json::Value::Object(Default::default()),
        }
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// Attach kind-specific structured data (read by the FE renderer).
    pub fn payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }

    /// Durable inbox row only — no live toast.
    pub fn silent(mut self) -> Self {
        self.interrupt = false;
        self
    }
}

/// Paged list response.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct NotificationPage {
    pub items: Vec<Notification>,
    pub total: i64,
    pub unread: i64,
    pub page: i64,
    pub per_page: i64,
}

/// Unread-count response.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct UnreadCount {
    pub unread: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_sets_fields_and_defaults() {
        let uid = Uuid::new_v4();
        let n = NewNotification::new(uid, "study_share_invite", "Shared")
            .body("Alice shared Study X")
            .payload(serde_json::json!({"study_id": "abc"}));
        assert_eq!(n.user_id, uid);
        assert_eq!(n.kind, "study_share_invite");
        assert_eq!(n.title, "Shared");
        assert_eq!(n.body, "Alice shared Study X");
        assert!(n.interrupt, "interrupts by default");
        assert_eq!(n.payload["study_id"], "abc");

        let silent = NewNotification::new(uid, "k", "t").silent();
        assert!(!silent.interrupt);
        assert_eq!(
            NewNotification::new(uid, "k", "t").payload,
            serde_json::json!({})
        );
    }

    #[test]
    fn is_unread_reflects_read_at() {
        let mut n = Notification {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            kind: "k".into(),
            title: "t".into(),
            body: String::new(),
            interrupt: true,
            payload: serde_json::json!({}),
            read_at: None,
            created_at: Utc::now(),
        };
        assert!(n.is_unread());
        n.read_at = Some(Utc::now());
        assert!(!n.is_unread());
    }

    #[test]
    fn notification_serde_roundtrips_including_payload() {
        let n = Notification {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            kind: "study_share_invite".into(),
            title: "Shared".into(),
            body: "Alice shared Study X".into(),
            interrupt: true,
            payload: serde_json::json!({ "study_id": "abc", "share_id": 7 }),
            read_at: None,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&n).unwrap();
        let back: Notification = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, n.id);
        assert_eq!(back.kind, "study_share_invite");
        assert_eq!(back.payload["study_id"], "abc");
        assert_eq!(back.payload["share_id"], 7);
        assert!(back.is_unread());
    }

    #[test]
    fn page_and_unread_count_serialize_expected_fields() {
        let page = NotificationPage {
            items: vec![],
            total: 3,
            unread: 1,
            page: 2,
            per_page: 20,
        };
        let v = serde_json::to_value(&page).unwrap();
        assert_eq!(v["total"], 3);
        assert_eq!(v["unread"], 1);
        assert_eq!(v["page"], 2);
        assert_eq!(v["per_page"], 20);
        assert!(v["items"].is_array());

        let uc = serde_json::to_value(UnreadCount { unread: 5 }).unwrap();
        assert_eq!(uc["unread"], 5);
    }
}
