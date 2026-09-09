-- #251 — `users.email` uniqueness must be case-insensitive.
--
-- `users_email_key` (`UNIQUE (email)`, added in 202607140050_auth_schema.sql:157)
-- is byte-exact, so `bob@corp.com` and `BOB@CORP.COM` were two DISTINCT
-- principals. `POST /api/auth/register` is open self-registration, so anyone
-- holding a leaked invitation link for `bob@corp.com` could register the case
-- variant and satisfy an accept path that binds the redeemer on
-- `lower(trim(users.email))` — a total bypass of the recipient control.
--
-- ── why this is a CHECK and NOT a `UNIQUE (lower(email))` index ──────────────
--
-- A functional unique index has to have a NAME, and reserving one is what made
-- two P1 security bugs possible in the earlier attempts at this fix:
--
--   * #283 — a unique violation attributed by matching the constraint NAME is
--     MIS-attributed the moment a same-shape index exists under another name
--     (lower OID wins), turning an unauthenticated OAuth callback's 409 into a
--     500 and reopening an existence oracle for deactivated accounts;
--   * #284 — Postgres index names are unique per SCHEMA, not per table, so the
--     `DROP INDEX IF EXISTS public.<reserved name>` used to guarantee "the
--     enforcing index is always exactly ours" silently destroyed a same-named
--     index on an UNRELATED table, and never recreated it.
--
-- Both are properties of the NAME. This migration reserves none, so both are
-- dissolved rather than patched: there is nothing to attribute a violation by
-- and nothing to drop. The existing plain `UNIQUE (email)` is sufficient ON
-- NORMALISED DATA, and the CHECK is what makes the data normalised — not as a
-- convention the writers observe, but as an invariant the database holds. A
-- future write path that forgets to lowercase fails loudly with a 23514
-- instead of silently creating a second principal.
--
-- Nothing is dropped, renamed or replaced here. `idx_users_lower_email` (a
-- NON-unique btree on `lower(email)` from the base schema) is left exactly as
-- it is: it accelerates the case-insensitive lookups and enforces nothing.
--
-- ── the Unicode seam (#260) ─────────────────────────────────────────────────
--
-- The writers lowercase in RUST; this CHECK folds in POSTGRES. The two folds
-- disagree on 56 code points (`lower('İ')` is one character in Postgres and
-- two in Rust), and `lower()` is collation-driven, so a C-locale cluster does
-- not fold non-ASCII at all. Over PRINTABLE ASCII they agree exactly and are
-- collation-invariant, which is why `auth::email::normalize_email` REFUSES
-- anything outside printable ASCII at every write path — that restriction is
-- what makes this CHECK satisfiable by construction, and it is asserted (not
-- assumed) by `tests/email_case_invariant.rs`.
--
-- #260 tracks admitting non-ASCII addresses properly. Do not widen the
-- accepted character set without closing it: this CHECK is what breaks first,
-- on the register path, as a 500.
--
-- ── step 1: REFUSE to migrate data a human has to adjudicate ────────────────
--
-- Two rows differing only by case are two accounts. Merging or deleting either
-- one silently is a data-loss decision this migration has no standing to make
-- — and picking a winner is exactly the privilege-escalation the issue is
-- about. So it aborts, naming the colliding addresses, and an operator decides
-- which account survives before re-running.
DO $$
DECLARE
    collisions text;
BEGIN
    SELECT string_agg(detail, E'\n  ' ORDER BY detail)
      INTO collisions
      FROM (
          SELECT lower(email) || ' <- ' || string_agg(email, ', ' ORDER BY email)
                 AS detail
            FROM public.users
           GROUP BY lower(email)
          HAVING count(*) > 1
      ) AS dupes;

    IF collisions IS NOT NULL THEN
        RAISE EXCEPTION
            'ziee-auth 202609090010 (#251): users.email holds rows that differ ONLY by case, so they cannot all be lowercased. Decide which account survives, then re-run. Colliding addresses: %',
            collisions
            USING HINT = 'Each entry is: canonical address <- the variants currently stored. Merge or remove all but one row per entry (see issue #251).';
    END IF;
END
$$;

-- ── step 2: normalise the rows that are left ────────────────────────────────
--
-- Postgres's `lower()` here, Rust's ASCII fold at the writers. For any row
-- that a writer could have produced they are identical; for a legacy non-ASCII
-- row Postgres's answer is the one the CHECK will judge it by, so it is also
-- the right one to store. (Such a row stays readable and loginable; only an
-- attempt to REWRITE its address goes through the ASCII gate, which refuses.)
--
-- Touching a row fires `update_users_updated_at`, so migrated rows get a fresh
-- `updated_at`. The `WHERE` keeps that to the rows that actually change.
UPDATE public.users
   SET email = lower(email)
 WHERE email <> lower(email);

-- ── step 3: make it an invariant ────────────────────────────────────────────
--
-- CHECK constraint names live in `pg_constraint` keyed by (table, name), and
-- `ALTER TABLE` is inherently table-qualified — so unlike an index name, this
-- name cannot collide with, shadow, or be dropped in place of an object
-- belonging to another table. Nothing in Rust matches on it; a 23514 from here
-- means a writer skipped `auth::email::normalize_email`, and it should surface
-- as the loud, un-mapped failure it is.
ALTER TABLE public.users
    ADD CONSTRAINT users_email_is_lowercase CHECK (email = lower(email));
