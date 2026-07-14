-- user_groups.assigned_by FK: add ON DELETE SET NULL.
--
-- The initial schema (migration 1) declared `assigned_by UUID REFERENCES
-- users(id)` with no ON DELETE action, so it defaulted to NO ACTION/RESTRICT.
-- That made deleting a user who is recorded as `assigned_by` on any
-- user_groups row fail with a FK violation — a latent "cannot delete this
-- user" bug. The column is nullable, so SET NULL (drop the attribution) is the
-- correct ownership semantics for an attributive FK.
--
-- The original constraint is unnamed, so it carries Postgres's default name
-- `user_groups_assigned_by_fkey`. Migration 1's body is NOT edited (that would
-- break the sqlx checksum); we ALTER here instead.
ALTER TABLE user_groups DROP CONSTRAINT user_groups_assigned_by_fkey;
ALTER TABLE user_groups
    ADD CONSTRAINT user_groups_assigned_by_fkey
    FOREIGN KEY (assigned_by) REFERENCES users(id) ON DELETE SET NULL;
