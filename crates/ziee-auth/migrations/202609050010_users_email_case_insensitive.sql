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

-- 3. The collision ledger. PERMANENT, not a migration scratch table: it is the ONLY record of
--    what step 4 changed, and an operator needs it to reinstate an address that step 4 parked.
--
--    An earlier version of step 4 wrote the originals to `RAISE NOTICE` and called that "the
--    operator can review and reinstate". A blind audit measured what that is worth: Postgres
--    does not write NOTICE to the server log at the default `log_min_messages = WARNING`, so
--    the originals existed NOWHERE afterwards -- a silent destructive rewrite of user
--    identifiers. The same NOTICE was also a bulk dump of user email addresses into whatever
--    consumes Postgres notices; the notices below now name a user id and this table, never an
--    address.
CREATE TABLE IF NOT EXISTS public.users_email_collision_log (
    user_id        uuid PRIMARY KEY REFERENCES public.users(id) ON DELETE CASCADE,
    original_email character varying(255) NOT NULL,
    parked_email   character varying(255) NOT NULL,
    detected_at    timestamp with time zone NOT NULL DEFAULT now()
);

COMMENT ON TABLE public.users_email_collision_log IS
    'Issue #251. One row per user whose address was PARKED when case-insensitive uniqueness '
    'was introduced, because two accounts claimed one mailbox. The account is untouched and '
    'still active -- only its email was moved aside. Reinstate with: UPDATE users u SET email '
    '= l.original_email FROM users_email_collision_log l WHERE u.id = l.user_id AND ... , '
    'after deciding which account owns the address.';

-- 4. Resolve pre-existing collisions. DETERMINISTIC, NON-DESTRUCTIVE and NON-FATAL: this
--    migration must never fail at deploy on data the bug itself created, and it must not
--    make an unrecoverable judgement about which of two accounts is the real one.
--
--    WHAT AN EARLIER VERSION DID, AND WHY IT WAS WRONG -- all three reproduced by a blind
--    audit against a live cluster, so they are recorded rather than paraphrased:
--
--      * It ranked by `created_at` alone and DEACTIVATED every later collider. In the exact
--        attack #251 describes, the squatter registers the case variant FIRST -- so the
--        migration awarded the mailbox to the ATTACKER and disabled the legitimate account.
--      * `is_admin` was not consulted, so the ROOT ADMIN could be the deactivated row. That
--        bricks the deployment: `has_admin()` has no `is_active` filter (first-run setup
--        still refuses with SETUP_ALREADY_COMPLETE) and the `unique_root_admin` partial index
--        blocks promoting anyone else. Recovery required direct DB surgery.
--      * Deactivating a collider could leave an organization account with no administrable
--        owner, and could disable an innocent bystander whose address happened to equal the
--        parking target.
--
--    SO: nothing is deactivated, nothing is deleted, and no address is lost.
--
--    RANKING. The row that KEEPS the address is chosen by evidence, not by luck:
--      1. `is_admin` -- never park the administrator's address; the lockout above is not
--         recoverable in-app.
--      2. `email_verified` -- the only real evidence of mailbox control in this schema. It is
--         set by OAuth provisioning after a provider asserted the address, so where exactly
--         one collider has it, the choice is not a guess at all.
--      3. `created_at`, then `id` -- a deterministic tiebreak, and ONLY a tiebreak. Where it
--         decides, the NOTICE says so, because registration order is not evidence.
--
--    PARKING TARGET. `dup.<user_id>.<run_id>@invalid`, where `run_id` is generated ONCE per
--    migration run. An earlier version used `dup.<user_id>@invalid` and looped to re-check,
--    because a pre-existing row could already hold that literal string; the audit showed the
--    loop then never converges and aborts the migration permanently (every retry rolls back
--    and fails identically). Mixing in a per-run uuid makes the target unguessable by any
--    pre-existing data, so ONE pass suffices and the loop is gone. `.invalid` is RFC 2606
--    reserved, so a parked address can never be deliverable.
DO $$
DECLARE
    r      RECORD;
    run_id uuid := gen_random_uuid();
    parked character varying(255);
BEGIN
    FOR r IN
        SELECT id, email, is_admin, email_verified, decided_by_order
          FROM (SELECT id,
                       email,
                       is_admin,
                       email_verified,
                       row_number() OVER (PARTITION BY lower(email)
                                              ORDER BY is_admin DESC,
                                                       email_verified DESC,
                                                       created_at,
                                                       id) AS rn,
                       -- True when neither is_admin nor email_verified separated this row
                       -- from the winner, i.e. registration order alone decided.
                       (count(*) FILTER (WHERE is_admin) OVER (PARTITION BY lower(email)) = 0
                        AND count(*) FILTER (WHERE email_verified) OVER (PARTITION BY lower(email)) = 0)
                           AS decided_by_order
                  FROM public.users) g
         WHERE g.rn > 1
    LOOP
        parked := 'dup.' || r.id::text || '.' || run_id::text || '@invalid';

        INSERT INTO public.users_email_collision_log (user_id, original_email, parked_email)
        VALUES (r.id, r.email, parked)
        ON CONFLICT (user_id) DO NOTHING;

        UPDATE public.users
           SET email      = parked,
               updated_at = now()
         WHERE id = r.id;

        IF r.decided_by_order THEN
            RAISE NOTICE
                '202609050010: two accounts claimed one mailbox and NEITHER is an admin or has a verified address, so registration order decided. User % keeps its account (still active) but its address was parked; the original is in users_email_collision_log. REVIEW THIS.',
                r.id;
        ELSE
            RAISE NOTICE
                '202609050010: two accounts claimed one mailbox. User % keeps its account (still active) but its address was parked; the original is in users_email_collision_log.',
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
