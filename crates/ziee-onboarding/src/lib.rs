//! `ziee-onboarding` — a generic, domain-free guide/step completion tracker
//! (schema-bound SDK module crate).
//!
//! Moved wholesale from ziee's `modules::onboarding` (chunk sdk-surfaces): the
//! DB-free wire type ([`OnboardingProgress`]) + the schema-bound repository
//! ([`OnboardingRepository`], compile-time `query!`/`query_as!` over
//! `user_onboarding` ONLY) + the module's `user_onboarding` table migration. The
//! completion keys are arbitrary strings (`"{guide_id}"` / `"{guide_id}/{step_id}"`)
//! — the crate has zero knowledge of any concrete guide; the CONCRETE guides +
//! step content are frontend (app-side, deferred to the shell wave), and the
//! sync-coupled handlers/routes/registration stay in ziee (they name
//! `SyncEntity::Onboarding` / `Repos` / `RequirePermissions` / `JwtAuth`).
//!
//! ziee consumes this via equivalence-preserving re-export shims: its
//! `modules::onboarding` re-exports [`OnboardingRepository`] +
//! [`models::OnboardingProgress`], so `Repos.onboarding`, the handlers, and the
//! OpenAPI response schema (`OnboardingProgress`, keyed by short ident) are all
//! byte-unchanged.

pub mod models;
pub mod repository;

pub use models::OnboardingProgress;
pub use repository::OnboardingRepository;
