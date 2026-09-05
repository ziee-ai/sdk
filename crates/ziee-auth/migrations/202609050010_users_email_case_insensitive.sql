-- Case-INSENSITIVE uniqueness for `users.email` (issue #251).
--
-- WHAT WAS WRONG
--
-- `202607140050_auth_schema.sql` created `users_email_key UNIQUE (email)`, a CASE-SENSITIVE
-- constraint. So `bob@corp.com` and `BOB@corp.com` were two distinct principals, and BOTH
-- satisfied an account invitation issued to `bob@corp.com` (whose binding normalizes with
-- `lower(trim(...))`). Registration is open and unverified, so anyone holding a leaked
-- invite link could register the case variant and redeem it.
--
-- WHAT THIS DOES, AND THE ONE RULE THAT MATTERS
--
-- Uniqueness becomes `UNIQUE (lower(email))`. The case-fold is Postgres's `lower()`, in the
-- index AND on both sides of every lookup inside this crate; Rust never case-folds an
-- address here. The trim is Rust's `str::trim`, at every writer; Postgres never trims in the
-- hot path. The two responsibilities are DISJOINT within `ziee-auth`, which is deliberate:
-- Postgres `lower('I-with-dot' U+0130)` yields ONE character (U+0069) while Rust's
-- `to_lowercase()` yields TWO (U+0069 U+0307), so any design requiring the two case-folds to
-- agree is unsound for at least that input.
--
-- THE LIMIT OF THAT CLAIM, STATED RATHER THAN GLOSSED
--
-- An earlier version of this header said the crossing therefore "does not exist". That was
-- false ACROSS TABLE BOUNDARIES and a blind audit demonstrated it. A consumer that stores a
-- RUST-lowercased address in its own table (cytoanalyst's `account_invitations.email`, via
-- `invitation_token::normalize_email`) and then matches it against `users.email` is doing
-- exactly the comparison this design avoids internally. A full `0..0x10FFFF` sweep of
-- Postgres `lower()` against Rust `char::to_lowercase` finds 56 divergent code points: the
-- two spellings `U+0130` and `U+0069 U+0307` are DIFFERENT under `lower()` (so both may be
-- stored here) yet IDENTICAL under `to_lowercase()` (so both would satisfy one binding).
--
-- For every address that is printable ASCII the two folds agree exactly, and cytoanalyst's
-- `validate_invitation_email` admits only printable ASCII -- so the divergence is not
-- reachable through any shipped route today, and that is asserted by a test rather than
-- claimed here. It is a real constraint on any consumer that widens its address charset, and
-- the fix for such a consumer is the same discipline: do not case-fold in Rust; compare with
-- `lower()` on both sides.
--
-- THE ONE CROSSING INSIDE THIS CRATE, AND WHY THE CHARACTER SET IS SPELLED OUT
--
-- Steps 2 and 6 below are the only place a Postgres expression must match Rust's trim. A
-- ONE-ARGUMENT `btrim` would strip ASCII SPACE ONLY, while `str::trim` strips all Unicode
-- `White_Space` -- and that gap IS the bypass in a narrower form: a `U+00A0`-padded address
-- would sit beside its unpadded twin as a second principal. So the character set is given
-- explicitly and is EXACTLY the 25 code points of Unicode `White_Space`, which is exactly the
-- set `char::is_whitespace` returns true for:
--
--   U+0009..U+000D, U+0020, U+0085, U+00A0, U+1680, U+2000..U+200A,
--   U+2028, U+2029, U+202F, U+205F, U+3000
--
-- Equality is required in BOTH directions and is machine-checked, not asserted. A set SMALLER
-- than Rust's leaves the bypass open for the missing code points; a set LARGER than Rust's
-- makes the step-6 CHECK reject a value the Rust writer legitimately produced, turning a legal
-- registration into a 500. `auth::email`'s `migration_charset_equals_rust_trim_set` test reads
-- THIS FILE, extracts these escapes, and fails if the set differs from `char::is_whitespace`
-- by even one code point -- so a future Unicode revision is a red test, not a silent hole.
--
-- DATABASE ENCODING REQUIREMENT
--
-- The escape-string literals below carry code points above U+007F, which are only
-- representable when the database
-- encoding is UTF8; Postgres rejects them outright on a SQL_ASCII or LATIN1 cluster, so this
-- migration will not apply there. That is stated rather than worked around, because such a
-- cluster cannot STORE a `U+3000`-padded address either -- the constraint it would be asked
-- to enforce is not expressible in that encoding. A guard statement cannot help: Postgres
-- parses the whole migration before executing any of it, so the parse error precedes any
-- check we could write. Deployments must use a UTF8 database (which is what the embedded
-- Postgres and the app's own `CREATE DATABASE` produce by default). Tracked separately.
--
-- COLLATION
--
-- `lower()` is collation-driven. ASCII case folding is collation-INVARIANT, so the reachable
-- domain above is unaffected; for non-ASCII addresses the fold (and therefore this index)
-- depends on the cluster's locale and can differ across a glibc/ICU upgrade or a restore into
-- a differently-collated cluster. That is the same residual as the paragraph above and is
-- tracked with it, not silently inherited.
--
-- WHAT THIS DOES NOT DO
--
-- No provider-specific canonicalization (`a.b@gmail` vs `ab@gmail`, `+tag` suffixes): case
-- folding is the only normalization considered safe across providers (RFC 5321 SS2.4
-- discourages relying on local-part case sensitivity, and every major provider case-folds).
-- The stored address keeps its ORIGINAL CASING -- lowercasing happens only in comparisons --
-- because the display and SMTP forms want the address the user typed.

-- 1. Drop the CASE-SENSITIVE constraint FIRST.
--
--    Ordering here is load-bearing, and getting it wrong is a deploy-time failure rather
--    than a cosmetic issue -- it was caught by
--    `migration_resolves_preexisting_collisions_without_failing`, which builds the collision
--    state deliberately. With `users_email_key` still in force, step 2's whitespace UPDATE
--    normalizes a `U+00A0`-padded twin onto its unpadded original and trips the OLD
--    constraint (`23505 users_email_key`), aborting the migration on exactly the data this
--    migration exists to repair. Steps 2 and 4 need a window with NO uniqueness rule on
--    `email`; sqlx applies each migration inside a transaction, so that window is never
--    visible to anyone else.
--
--    Dropping rather than keeping: `UNIQUE (lower(email))` strictly implies `UNIQUE (email)`,
--    so retaining it would only cost a redundant index on every write, and leaving it in
--    place would let the old, weaker rule keep reading as if it were still the rule.
--    Referenced by no `ON CONFLICT` clause and no Rust code (grepped tree-wide).
ALTER TABLE public.users DROP CONSTRAINT IF EXISTS users_email_key;

-- 2. Normalize whitespace on pre-existing rows, so storage matches what the writers now
--    produce. Runs BEFORE the collision pass so a padded twin collides with its unpadded
--    original and is resolved by step 4 rather than surviving as a second principal.
UPDATE public.users
   SET email = btrim(email, E'\u0009\u000A\u000B\u000C\u000D\u0020\u0085\u00A0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200A\u2028\u2029\u202F\u205F\u3000'),
       updated_at = now()
 WHERE email <> btrim(email, E'\u0009\u000A\u000B\u000C\u000D\u0020\u0085\u00A0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200A\u2028\u2029\u202F\u205F\u3000');

-- 3. The collision ledger. PERMANENT and APPEND-ONLY, not a migration scratch table: it is
--    the ONLY record of what step 4 changed, and an operator needs it to reinstate an
--    address that step 4 parked.
--
--    An earlier version of step 4 wrote the originals to `RAISE NOTICE` and called that "the
--    operator can review and reinstate". A blind audit measured what that is worth: Postgres
--    does not write NOTICE to the server log at the default `log_min_messages = WARNING`, so
--    the originals existed NOWHERE afterwards -- a silent destructive rewrite of user
--    identifiers. The notices below now name a user id and this table, never an address.
--
--    APPEND-ONLY, with a surrogate key rather than `user_id` as the PK. A second audit round
--    showed why: one user can legitimately need TWO rows -- parked, then reinstated by an
--    operator under a corrected address, then parked again by a later collision -- and a
--    `user_id` primary key with `ON CONFLICT DO NOTHING` silently DROPPED the second record,
--    leaving the ledger asserting a stale original. Following the table comment's recipe then
--    restored a DIFFERENT PERSON'S address. A ledger that can lose a row is not a ledger.
CREATE TABLE IF NOT EXISTS public.users_email_collision_log (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id             uuid NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    original_email      character varying(255) NOT NULL,
    parked_email        character varying(255) NOT NULL,
    -- FALSE when no evidence separated this group and the addresses were parked because the
    -- migration REFUSED to guess -- the rows an operator must actually adjudicate. It lives
    -- here, not only in a NOTICE, for exactly the reason this table exists at all.
    decided_by_evidence boolean NOT NULL,
    detected_at         timestamp with time zone NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_users_email_collision_log_user
    ON public.users_email_collision_log (user_id);

COMMENT ON TABLE public.users_email_collision_log IS
    'Issue #251, append-only. One row per PARKING of a user address when case-insensitive '
    'uniqueness was introduced and two accounts claimed one mailbox. The account is '
    'untouched and still active -- only its email was moved aside. Rows with '
    'decided_by_evidence = false are the ones a human must adjudicate: no account in that '
    'group was an admin, had a verified address, or was uniquely active, so EVERY address in '
    'it was parked rather than awarded on registration order. Reinstate the correct one with: '
    'UPDATE users SET email = (SELECT original_email FROM users_email_collision_log '
    'WHERE user_id = users.id ORDER BY detected_at DESC LIMIT 1) WHERE id = ...';

-- 4. Resolve pre-existing collisions. DETERMINISTIC, NON-DESTRUCTIVE, NON-FATAL, and -- the
--    property two audit rounds were needed to get right -- UNGAMEABLE.
--
--    WHAT EARLIER VERSIONS DID, AND WHY THEY WERE WRONG. All reproduced by blind audits
--    against a live cluster, so they are recorded rather than paraphrased:
--
--      * v1 ranked by `created_at` alone and DEACTIVATED every later collider. In the exact
--        attack #251 describes the squatter registers the case variant FIRST, so the
--        migration awarded the mailbox to the ATTACKER and disabled the legitimate account.
--        Where that account was the root admin it bricked the deployment: `has_admin()` has
--        no `is_active` filter (first-run setup still refuses) and `unique_root_admin` blocks
--        promoting anyone else.
--      * v2 added `is_admin` and `email_verified` to the ranking and stopped deactivating --
--        but still FELL THROUGH TO `created_at` when neither applied, which is the NORMAL
--        case: registration is open and unverified, and `email_verified` is only ever set by
--        OAuth provisioning. So on precisely the attack data this migration exists to repair,
--        the squatter still won the mailbox -- and the new unique index made it PERMANENT,
--        because the victim could no longer take the address back through any route.
--
--    THE RULE NOW: evidence decides, or NOBODY does.
--
--    Each row scores its evidence of being the legitimate holder:
--      4  `is_admin`        -- never park the administrator; the lockout above is not
--                              recoverable in-app.
--      2  `email_verified`  -- the only real proof of mailbox control in this schema, set by
--                              OAuth provisioning after a provider asserted the address.
--      1  `is_active`       -- a disabled account must not outrank a live one. (v2 omitted
--                              this, and a BANNED squatter kept the mailbox over an active
--                              legitimate user.)
--
--    If exactly ONE row in the group holds the maximum score, it keeps the address: that is a
--    decision made on evidence, not a guess. If the maximum is TIED -- including the common
--    case where nothing distinguishes anyone -- then EVERY address in the group is parked and
--    the ledger flags the group for review.
--
--    Parking everyone is deliberately fail-CLOSED. Registration order is not evidence, and a
--    collision here IS the artifact of the attack being fixed, so awarding the mailbox on it
--    hands an attacker a permanent binding that the new unique index then protects. Nobody
--    holding it is recoverable in one UPDATE; the wrong person holding it is not recoverable
--    at all. The losing accounts keep their password and username login and lose only the
--    address.
--
--    PARKING TARGET: `dup.<user_id>.<run_id>@invalid`, where `run_id` is generated ONCE per
--    migration run. v1 used `dup.<user_id>@invalid` and looped to re-check, because a
--    pre-existing row could already hold that literal string; the audit showed the loop then
--    never converges and aborts the migration permanently (every retry rolls back and fails
--    identically). Mixing in a per-run uuid makes the target unguessable by any pre-existing
--    data, so ONE pass suffices and the loop is gone. `.invalid` is RFC 2606 reserved, so a
--    parked address can never be deliverable.
DO $$
DECLARE
    r      RECORD;
    run_id uuid := gen_random_uuid();
    parked character varying(255);
BEGIN
    FOR r IN
        SELECT id, email, (top_n = 1) AS decided_by_evidence
          FROM (SELECT id,
                       email,
                       score,
                       top_score,
                       group_n,
                       count(*) FILTER (WHERE score = top_score)
                           OVER (PARTITION BY k) AS top_n
                  FROM (SELECT id,
                               email,
                               lower(email) AS k,
                               (is_admin::int * 4
                                + email_verified::int * 2
                                + is_active::int) AS score,
                               max(is_admin::int * 4
                                   + email_verified::int * 2
                                   + is_active::int)
                                   OVER (PARTITION BY lower(email)) AS top_score,
                               count(*) OVER (PARTITION BY lower(email)) AS group_n
                          FROM public.users) scored) ranked
         WHERE group_n > 1
           -- Park every row that is not the SOLE holder of the top score. When the top score
           -- is tied, `top_n > 1` parks the tied rows too -- including the winner-by-order
           -- that earlier versions would have crowned.
           AND (score < top_score OR top_n > 1)
    LOOP
        parked := 'dup.' || r.id::text || '.' || run_id::text || '@invalid';

        INSERT INTO public.users_email_collision_log
            (user_id, original_email, parked_email, decided_by_evidence)
        VALUES (r.id, r.email, parked, r.decided_by_evidence);

        UPDATE public.users
           SET email      = parked,
               updated_at = now()
         WHERE id = r.id;

        IF r.decided_by_evidence THEN
            RAISE NOTICE
                '202609050010: two accounts claimed one mailbox; another had stronger evidence. User % keeps its account (still active) but its address was parked; the original is in users_email_collision_log.',
                r.id;
        ELSE
            RAISE NOTICE
                '202609050010: two accounts claimed one mailbox and NOTHING distinguished them, so NEITHER was awarded it. User % keeps its account (still active) but its address was parked; the original is in users_email_collision_log with decided_by_evidence = false. REVIEW THIS.',
                r.id;
        END IF;
    END LOOP;
END
$$;

-- 5. The fix. Two addresses differing only by case are now ONE principal, enforced by the
--    DATABASE rather than by an application pre-check that a race can slip past.
--
--    Named `..._unique_idx`, not `..._key`: Postgres reserves the `_key` suffix by convention
--    for UNIQUE CONSTRAINTS created via ALTER TABLE, and this is necessarily a bare index (an
--    expression cannot back a UNIQUE constraint). A `_key` name would be absent from
--    `pg_constraint` while reading as if it were there, and `ON CONFLICT ON CONSTRAINT` would
--    fail at runtime.
CREATE UNIQUE INDEX IF NOT EXISTS users_email_lower_unique_idx
    ON public.users (lower(email));

-- 5b. `202607140050` already created `idx_users_lower_email` on the byte-identical expression
--     `lower(email)`. It is now strictly redundant -- every write would maintain two identical
--     btrees -- so it goes.
DROP INDEX IF EXISTS public.idx_users_lower_email;

-- 6. Defence in depth: the stored address is trimmed. Every in-crate writer trims in Rust
--    before it gets here, so this CHECK is unreachable through any shipped route; its job is
--    to make a FUTURE writer that forgets fail LOUDLY at the insert, rather than silently
--    reintroducing #251 through a leading/trailing `U+00A0` variant. See the header for why
--    the character set is enumerated and how the enumeration is kept honest.
ALTER TABLE public.users DROP CONSTRAINT IF EXISTS users_email_trimmed;
ALTER TABLE public.users
    ADD CONSTRAINT users_email_trimmed CHECK (email = btrim(email, E'\u0009\u000A\u000B\u000C\u000D\u0020\u0085\u00A0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200A\u2028\u2029\u202F\u205F\u3000'));
