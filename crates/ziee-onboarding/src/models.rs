// Onboarding response models

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Per-user onboarding progress. Step ids use the composite
/// "{guide_id}/{step_id}" key format. Replaces the two columns that
/// previously rode on the `User` object.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OnboardingProgress {
    pub completed_guide_ids: Vec<String>,
    pub completed_step_ids: Vec<String>,
}
