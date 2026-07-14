//! Data types for the notification inbox: the `notifications` row, the insert
//! shape, and paged-list query params. Mirrors `mcp/tool_calls` shapes.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A row of `notifications`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, sqlx::FromRow)]
pub struct Notification {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub title: String,
    pub body: String,
    /// TRUE => client may toast on arrival; FALSE => durable inbox row only.
    pub interrupt: bool,
    pub scheduled_task_id: Option<Uuid>,
    pub workflow_run_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
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
    pub scheduled_task_id: Option<Uuid>,
    pub workflow_run_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
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
            scheduled_task_id: None,
            workflow_run_id: None,
            conversation_id: None,
        }
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }
    /// Durable inbox row only — no live toast (a 'silent' task's result).
    pub fn silent(mut self) -> Self {
        self.interrupt = false;
        self
    }
    pub fn task(mut self, id: Uuid) -> Self {
        self.scheduled_task_id = Some(id);
        self
    }
    pub fn workflow_run(mut self, id: Uuid) -> Self {
        self.workflow_run_id = Some(id);
        self
    }
    pub fn conversation(mut self, id: Uuid) -> Self {
        self.conversation_id = Some(id);
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
