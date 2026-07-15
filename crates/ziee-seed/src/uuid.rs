//! Deterministic-UUID primitive for seed providers.
//!
//! An app provider that wants stable, reproducible primary keys for the rows it seeds
//! (so the same natural key always maps to the same UUID across fresh installs, and an
//! `ON CONFLICT (id) DO UPDATE` upsert is idempotent) derives them with [`seed_uuid`]
//! — a v5 UUID over a namespace + a natural key. This is the same idiom the built-in
//! loopback MCP servers use for their deterministic ids. Using it is OPTIONAL: the
//! `seed_ledger` already provides natural-key idempotency for providers that let the DB
//! generate random ids.

use uuid::Uuid;

/// A stable namespace for seed-derived UUIDs (the URL namespace — a fixed, well-known
/// v5 base). An app may pass its own per-domain namespace to [`seed_uuid`] instead.
pub const SEED_NAMESPACE: Uuid = Uuid::NAMESPACE_URL;

/// Derive a deterministic v5 UUID from `namespace` + `key`. The same `(namespace, key)`
/// always yields the same UUID.
pub fn seed_uuid(namespace: &Uuid, key: &str) -> Uuid {
    Uuid::new_v5(namespace, key.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_uuid_is_deterministic_and_key_sensitive() {
        let a = seed_uuid(&SEED_NAMESPACE, "widgets/alpha");
        let b = seed_uuid(&SEED_NAMESPACE, "widgets/alpha");
        let c = seed_uuid(&SEED_NAMESPACE, "widgets/beta");
        assert_eq!(a, b, "same namespace+key ⇒ same UUID");
        assert_ne!(a, c, "different key ⇒ different UUID");
        assert_eq!(a.get_version_num(), 5);
    }
}
