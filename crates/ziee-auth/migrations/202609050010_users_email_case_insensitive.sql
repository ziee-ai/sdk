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
-- Steps 2 and 5 below are the only place a Postgres expression must match Rust's trim. A
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
-- makes the trim CHECK reject a value the Rust writer legitimately produced, turning a legal
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
--    than a cosmetic issue -- it is caught by
--    `migration_refuses_a_whitespace_padded_twin_as_a_collision`, which goes RED if this
--    DROP is moved below step 2. With `users_email_key` still in force, step 2's UPDATE
--    normalizes a `U+00A0`-padded twin onto its unpadded original and trips the OLD
--    constraint (`23505 users_email_key`), aborting the migration on exactly the data this
--    migration exists to repair -- and with a Postgres duplicate-key error rather than the
--    diagnostic step 3 exists to give. Step 2 needs a window with NO uniqueness rule on
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

-- 3. REFUSE to proceed if two accounts already claim one mailbox.
--
--    THIS IS THE RESOLUTION, and it is a deliberate reversal of two earlier attempts. The
--    reasoning is recorded here rather than in a run log because the obvious reading of this
--    block -- "it just fails" -- is the wrong one.
--
--    Three blind audit rounds each broke a different AUTOMATIC resolution, every one
--    reproduced against a live cluster:
--
--      v1  rank by `created_at`, DEACTIVATE the losers.
--          In the exact attack #251 describes the squatter registers the case variant FIRST,
--          so this awarded the mailbox to the ATTACKER and disabled the legitimate account.
--          Where that account was the root admin it BRICKED the deployment: `has_admin()` has
--          no `is_active` filter, so first-run setup still refuses, and `unique_root_admin`
--          blocks promoting anyone else.
--
--      v2  add `is_admin` / `email_verified` to the ranking, deactivate nobody.
--          Still fell through to `created_at` whenever neither applied -- which is the NORMAL
--          case, because registration is open and unverified and `email_verified` is only ever
--          set by OAuth provisioning. The squatter still won, and the new unique index made it
--          PERMANENT: the victim could no longer take the address back through any route.
--
--      v3  add `is_active`; award only on a SOLE top score, park every address on a tie.
--          `is_active` is not evidence of mailbox control -- it is routine administrative
--          state -- so this TRANSFERRED a mailbox from a suspended legitimate holder to an
--          active squatter, and marked it `decided_by_evidence = true` so no operator would
--          look. And the tie branch was worse than it reads: parking both FREES the address,
--          and the attacker who created the collision is the one party who knows to poll
--          registration for it. One unauthenticated registration before the upgrade was also
--          enough to force a chosen victim's address to be parked.
--
--    The pattern is not that three policies had bugs. It is that **which of two accounts owns
--    a mailbox is not derivable from this schema.** Every signal available here is either
--    controlled by the attacker (registration order, casing) or unrelated to mailbox control
--    (`is_active`). An automatic answer is therefore a guess, the attacker picks the inputs,
--    and a wrong guess is unrecoverable once the unique index below protects it.
--
--    So the migration stops and asks. That IS a stated resolution -- it is simply not an
--    automatic one: the operator is told exactly which accounts collide and exactly what to
--    run, nothing is mutated (the whole migration is one transaction, so the trim in step 2
--    rolls back with it), and the upgrade resumes the moment a human has adjudicated.
--
--    THE COST OF BLOCKING, NAMED RATHER THAN GLOSSED. Refusing is not free, and an earlier
--    version of this comment asserted its safety without weighing the downside. Registration
--    on a not-yet-upgraded deployment is public and unverified, so TWO anonymous requests --
--    `bob@corp.com` and `BOB@corp.com` -- pre-plant a collision that makes this block refuse.
--    Because the app runs migrations at startup, the result is not a degraded auth subsystem:
--    the SERVER DOES NOT BOOT, identically on every restart, until an operator adjudicates.
--    An attacker who knows an upgrade is coming can therefore deny the upgrade for the cost of
--    two HTTP requests, and choose the address it happens on.
--
--    That is accepted, with eyes open, because the alternative is worse in kind rather than in
--    degree. The three automatic rules above each hand the attacker a PERMANENT, SILENT
--    AUTHORIZATION outcome -- the squatter holding the contested mailbox, or the legitimate
--    admin deactivated -- which the new unique index then protects and which no operator is
--    told about. This refusal costs AVAILABILITY, loudly, recoverably, and in a way one
--    `UPDATE` fixes. A loud recoverable outage is a better failure than a silent permanent
--    compromise; and unlike the automatic rules, it cannot be arranged to favour the attacker.
--    The diagnostic below is what keeps the outage short, which is why it carries the ids and
--    the remediation in MESSAGE rather than in fields the client drops.
--
--    It is also not a routine state: `users_email_key` has enforced byte-exact uniqueness
--    since `202607140050`, so the ONLY way to hold two rows for one mailbox is a deliberate
--    case or whitespace variant -- the artifact of the attack this migration exists to close.
--    A deployment in that state has an active security incident, and blocking its upgrade
--    until a human looks is the correct outcome. The precheck across every reachable database
--    on the development cluster found ZERO collisions, so in practice this never fires.
--
--    Two residuals are stated rather than hidden: a whitespace-ONLY pre-existing address is
--    normalized by step 2 to the empty string (no shipped writer can produce one, and the
--    unique index then admits exactly one such row); and sqlx leaks its session advisory lock
--    when a migration fails, so a consumer that RETRIES the migrator on the SAME pool blocks
--    rather than re-reporting the refusal. The app is unaffected -- it runs the migrator once
--    and exits, so each restart gets a fresh pool.
--
--    The diagnostic names USER IDS, never addresses: an operator can resolve the collision
--    from ids, and a migration should not dump user email addresses into deploy logs.
DO $$
DECLARE
    colliding int;
    ids       text;
    extra     int;
BEGIN
    SELECT count(*)
      INTO colliding
      FROM public.users
     WHERE lower(email) IN (SELECT lower(email)
                              FROM public.users
                             GROUP BY lower(email)
                            HAVING count(*) > 1);

    IF colliding > 0 THEN
        -- Bounded: an unbounded `string_agg` produced a 3.8 MB single-line field at 100k
        -- colliding rows, which is unusable in any error surface.
        SELECT string_agg(id::text, ', ' ORDER BY id)
          INTO ids
          FROM (SELECT id
                  FROM public.users
                 WHERE lower(email) IN (SELECT lower(email)
                                          FROM public.users
                                         GROUP BY lower(email)
                                        HAVING count(*) > 1)
                 ORDER BY id
                 LIMIT 50) capped;
        extra := colliding - least(colliding, 50);

        -- EVERYTHING actionable goes in MESSAGE, not DETAIL/HINT.
        --
        -- That is the opposite of the idiomatic choice, and it is deliberate: three
        -- independent blind audits reproduced the same defect against the real runner.
        -- sqlx's `MigrateError`/`Error` `Display` impl renders ONLY Postgres's primary
        -- message (`PgDatabaseError::Display` is `f.write_str(self.message())`); DETAIL and
        -- HINT survive only under `{:?}`. The app's startup path prints the `Display` form
        -- (`main.rs`: `eprintln!("fatal: {e}")`), so an affected deployment previously saw a
        -- bare count and nothing else -- no ids, no remediation, no statement that nothing
        -- had been modified -- on a server that then refuses to boot. Structured fields are
        -- the right shape for a client that surfaces them; this client drops them.
        --
        -- The remediation names the ids DIRECTLY rather than handing over a discovery query.
        -- An earlier version's query omitted the whitespace `btrim` that step 2 applies, so
        -- it returned ZERO ROWS for a whitespace-only collision -- the operator was told to
        -- run something that could not find what the migration had just refused over. The
        -- ids are exact, need no charset, and cannot drift from the guard.
        RAISE EXCEPTION
            USING MESSAGE = format(
                      'MIGRATION 202609050010 STOPPED: %s user account(s) share a mailbox '
                      '(case-insensitively, or after trimming whitespace). This migration '
                      'will not guess which account owns an address -- see the comment above '
                      'this block for why every automatic rule was reproduced as an attack. '
                      'Affected users.id: %s%s. '
                      'Inspect with: SELECT id, username, email, is_admin, email_verified, '
                      'is_active, created_at FROM users WHERE id IN (<the ids above>); '
                      'then decide which account owns each address, change the OTHER '
                      'accounts to a different email (or remove them), and re-run. '
                      'NOTHING HAS BEEN MODIFIED: this migration is a single transaction '
                      'and has rolled back.',
                      colliding,
                      ids,
                      CASE WHEN extra > 0 THEN format(' (and %s more)', extra) ELSE '' END);
    END IF;
END
$$;

-- 4. The fix. Two addresses differing only by case are now ONE principal, enforced by the
--    DATABASE rather than by an application pre-check that a race can slip past.
--
--    DROP-THEN-CREATE, and NO GUARD. This is the third shape of this step, and the first two
--    are worth recording because the lesson is not about indexes.
--
--      v1  `CREATE UNIQUE INDEX IF NOT EXISTS`. That matches on NAME ONLY, so a pre-existing
--          index of this name made the statement a silent no-op: an audit pre-created one,
--          ran the migration, watched it report SUCCESS, and then inserted `bob@corp.com` and
--          `BOB@corp.com` side by side.
--      v2  the same, plus a guard asserting afterwards that a suitable index exists. The
--          first guard checked a NAME (evaded four ways). The second checked the PROPERTY and
--          was name-agnostic -- which introduced a worse failure: the index that SATISFIED
--          the guard could be `idx_users_lower_email`, which the very next statement DROPS.
--          Reproduced end to end: migration COMMITS reporting success, leaving a NON-unique
--          index, `users_email_key` already dropped by step 1, and three rows for one mailbox
--          -- #251 reopened AND the byte-exact uniqueness that existed beforehand lost.
--          Strictly worse than v1's guard, which would have refused that state.
--
--    Each guard was an attempt to DETECT a hazard introduced by `IF NOT EXISTS`. Removing
--    `IF NOT EXISTS` removes the hazard, and with it every evasion of every guard: an
--    unconditional DROP-then-CREATE cannot no-op, cannot be satisfied by an index something
--    else is about to drop, and needs nothing asserted about it afterwards. If a same-named
--    index already exists -- an operator's hand-rolled equivalent, or an impostor -- it is
--    replaced by the correct one, which is strictly better than refusing. If a CONSTRAINT
--    owns that name, `DROP INDEX` fails loudly rather than proceeding. And re-running the
--    file is still idempotent, which is why `IF NOT EXISTS` was reached for in the first place.
--
--    It also restores an invariant the name-agnostic guard had quietly broken: after this
--    step the enforcing index is ALWAYS `users_email_lower_unique_idx`, which is what lets
--    the Rust error mappers attribute a unique violation to the email rather than guessing.
--
--    Named `..._unique_idx`, not `..._key`: Postgres reserves the `_key` suffix by convention
--    for UNIQUE CONSTRAINTS created via ALTER TABLE, and this is necessarily a bare index (an
--    expression cannot back a UNIQUE constraint). A `_key` name would be absent from
--    `pg_constraint` while reading as if it were there.
DROP INDEX IF EXISTS public.users_email_lower_unique_idx;
CREATE UNIQUE INDEX users_email_lower_unique_idx ON public.users (lower(email));

-- 4b. `202607140050` already created `idx_users_lower_email` on the byte-identical expression
--     `lower(email)`. It is now strictly redundant -- every write would maintain two identical
--     btrees -- so it goes. Ordered AFTER step 4 deliberately: when a guard sat here instead,
--     the index it accepted could be the one this statement then destroyed.
DROP INDEX IF EXISTS public.idx_users_lower_email;

-- 5. Defence in depth: the stored address is trimmed. Every in-crate writer trims in Rust
--    before it gets here, so this CHECK is unreachable through any shipped route; its job is
--    to make a FUTURE writer that forgets fail LOUDLY at the insert, rather than silently
--    reintroducing #251 through a leading/trailing `U+00A0` variant. See the header for why
--    the character set is enumerated and how the enumeration is kept honest.
ALTER TABLE public.users DROP CONSTRAINT IF EXISTS users_email_trimmed;
ALTER TABLE public.users
    ADD CONSTRAINT users_email_trimmed CHECK (email = btrim(email, E'\u0009\u000A\u000B\u000C\u000D\u0020\u0085\u00A0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200A\u2028\u2029\u202F\u205F\u3000'));
