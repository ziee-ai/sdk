//! Wire types for the account-recovery endpoints.
//!
//! Nothing here ever carries a stored hash or a previously issued code: the
//! only place plaintext codes appear in a response is the generate/regenerate
//! reply, once.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which recovery capabilities this deployment offers. Public, so the sign-in
/// dialog knows whether to offer "Forgot password?" and with which methods.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RecoveryCapabilities {
    pub recovery_codes: bool,
    pub security_questions: bool,
}

/// A catalogue question as shown to a client.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QuestionPrompt {
    pub key: String,
    pub prompt: String,
}

/// `POST /auth/recovery/questions` body.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct QuestionsLookupRequest {
    pub username: String,
}

/// The questions to ask for a username. For a name that has none configured
/// this is a stable decoy list, indistinguishable in shape from a real one.
#[derive(Debug, Serialize, JsonSchema)]
pub struct QuestionsLookupResponse {
    pub questions: Vec<QuestionPrompt>,
}

/// How a reset proves the account is the caller's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMethod {
    /// One of the user's one-time recovery codes.
    Code,
    /// Answers to ALL of the user's security questions.
    Questions,
}

/// One answer to one configured question.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct QuestionAnswer {
    pub key: String,
    pub answer: String,
}

/// `POST /auth/recovery/reset` body.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResetPasswordRequest {
    pub username: String,
    pub method: RecoveryMethod,
    /// Required when `method` is `code`.
    #[serde(default)]
    pub code: Option<String>,
    /// Required when `method` is `questions`: every configured question.
    #[serde(default)]
    pub answers: Option<Vec<QuestionAnswer>>,
    pub new_password: String,
}

/// The signed-in user's recovery configuration.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RecoveryStatus {
    pub recovery_codes_enabled: bool,
    pub security_questions_enabled: bool,
    /// Unused codes left; `0` when none were ever generated.
    pub codes_remaining: i64,
    /// When the live code set was generated, if there is one.
    pub codes_generated_at: Option<DateTime<Utc>>,
    /// The questions the user configured (prompts only, never answers).
    pub questions: Vec<QuestionPrompt>,
    /// The catalogue the user may pick from (empty when questions are off).
    pub available_questions: Vec<QuestionPrompt>,
    /// Questions cannot be configured on this account (administrators).
    pub questions_blocked: bool,
    /// At least one method is configured: losing the password is recoverable.
    pub has_recovery: bool,
}

/// A body that only proves the caller still knows the password.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReauthRequest {
    pub current_password: String,
}

/// The freshly generated set. Shown once: nothing returns it again.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GeneratedCodes {
    pub codes: Vec<String>,
    pub generated_at: DateTime<Utc>,
}

/// One pick in a security-question set.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct QuestionPick {
    pub key: String,
    pub answer: String,
}

/// `PUT /auth/recovery/questions` body: the whole set, replacing any previous.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetQuestionsRequest {
    pub current_password: String,
    pub questions: Vec<QuestionPick>,
}
