-- ziee-seed: the seed_ledger — the single, table-agnostic record of WHICH rows the
-- declarative seed engine owns. Domain-neutral (names no app table): apps register their
-- own SeedProviders + data (decision N9); this table only records ownership by
-- `(section, natural_key)`.
--
-- `entity_id` is the owned row's UUID for multi-row families (used by dump +
-- reconcile-delete); NULL for singleton settings tables (whose identity IS the
-- natural_key). `first_seeded_at` doubles as the "already-seeded" latch: its presence
-- means the seed has taken ownership, so seed-if-empty leaves the row alone (and never
-- resurrects an admin-deleted default) — only a wipe (which drops this table) re-seeds,
-- and only `reconcile` mode re-syncs/deletes.
CREATE TABLE seed_ledger (
    section         TEXT NOT NULL,
    natural_key     TEXT NOT NULL,
    entity_id       UUID,
    first_seeded_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (section, natural_key)
);
