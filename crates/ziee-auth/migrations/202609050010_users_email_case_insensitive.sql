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
-- index AND on both sides of every lookup; Rust never case-folds an address. The trim is
-- Rust's `str::trim`, at every writer; Postgres never trims in the hot path. The two
-- responsibilities are DISJOINT, which is deliberate: Postgres `lower('I' U+0130)` yields one
-- character (U+0069) while Rust's `to_lowercase()` yields two (U+0069 U+0307), so any design
-- requiring the two case-folds to agree is unsound. Nothing here asks them to.
--
-- THE ONE CROSSING, AND WHY THE CHARACTER SET IS SPELLED OUT
--
-- Steps 1 and 5 below are the only place a Postgres expression must match Rust's trim. A
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
-- makes the step-5 CHECK reject a value the Rust writer legitimately produced, turning a legal
-- registration into a 500. `auth::email`'s `migration_charset_equals_rust_trim_set` test reads
-- THIS FILE, extracts these escapes, and fails if the set differs from `char::is_whitespace`
-- by even one code point -- so a future Unicode revision is a red test, not a silent hole.
--
-- WHAT THIS DOES NOT DO
--
-- No provider-specific canonicalization (`a.b@gmail` vs `ab@gmail`, `+tag` suffixes): case
-- folding is the only normalization considered safe across providers (RFC 5321 SS2.4 discourages
-- relying on local-part case sensitivity, and every major provider case-folds). The stored
-- address keeps its ORIGINAL CASING -- lowercasing happens only in comparisons -- because the
-- display and SMTP forms want the address the user typed.

-- 1. Drop the CASE-SENSITIVE constraint FIRST.
--
--    Ordering here is load-bearing, and getting it wrong is a deploy-time failure rather
--    than a cosmetic issue -- it was caught by
--    `migration_resolves_preexisting_collisions_without_failing`, which builds the collision
--    state deliberately. With `users_email_key` still in force, step 2's whitespace UPDATE
--    normalizes a `U+00A0`-padded twin onto its unpadded original and trips the OLD
--    constraint (`23505 users_email_key`), aborting the migration on exactly the data this
--    migration exists to repair. Steps 2 and 3 need a window with NO uniqueness rule on
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
--    original and is resolved by step 3 rather than surviving as a second principal.
UPDATE public.users
   SET email = btrim(email, E'\u0009\u000A\u000B\u000C\u000D\u0020\u0085\u00A0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200A\u2028\u2029\u202F\u205F\u3000'),
       updated_at = now()
 WHERE email <> btrim(email, E'\u0009\u000A\u000B\u000C\u000D\u0020\u0085\u00A0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200A\u2028\u2029\u202F\u205F\u3000');

-- 3. Resolve pre-existing case-insensitive collisions. DETERMINISTIC and NON-FATAL: this
--    migration must never fail at deploy on data the bug itself created.
--
--    Within each `lower(email)` group the EARLIEST `created_at` (ties by `id`) keeps the
--    address. Every later collider is deactivated and re-addressed to `dup.<id>@invalid`
--    (`.invalid` is RFC 2606 reserved, so it can never be deliverable). NO ROW IS EVER
--    DELETED and the original is written to the migration log, so an operator can review and
--    reinstate. Deactivating is the conservative reading: a case-insensitive collision IS two
--    accounts for one mailbox, which is the artifact of the defect being fixed here.
--
--    The rewrite is derived from the PRIMARY KEY, so two rewritten rows can never collide with
--    each other. The loop re-checks because a rewritten address could, in a contrived case,
--    equal a pre-existing literal `dup.<uuid>@invalid` row; that resolves on the next pass.
--    The iteration bound turns a non-converging (i.e. impossible) case into a clear diagnostic
--    instead of a hang.
DO $$
DECLARE
    r        RECORD;
    passes   int := 0;
    resolved int;
BEGIN
    LOOP
        resolved := 0;
        FOR r IN
            SELECT id, email
              FROM (SELECT id,
                           email,
                           row_number() OVER (PARTITION BY lower(email)
                                                  ORDER BY created_at, id) AS rn
                      FROM public.users) g
             WHERE g.rn > 1
        LOOP
            RAISE NOTICE
                '202609050010: case-insensitive duplicate email -- user % held %; deactivated and re-addressed to dup.%@invalid',
                r.id, r.email, r.id;
            UPDATE public.users
               SET email      = 'dup.' || r.id::text || '@invalid',
                   is_active  = false,
                   updated_at = now()
             WHERE id = r.id;
            resolved := resolved + 1;
        END LOOP;

        EXIT WHEN resolved = 0;

        passes := passes + 1;
        IF passes > 8 THEN
            RAISE EXCEPTION
                '202609050010: case-insensitive email collisions did not converge after 8 passes; resolve public.users by hand and re-run';
        END IF;
    END LOOP;
END
$$;

-- 4. The fix. Two addresses differing only by case are now ONE principal, enforced by the
--    DATABASE rather than by an application pre-check that a race can slip past.
CREATE UNIQUE INDEX users_email_lower_key ON public.users (lower(email));

-- 5. Defence in depth: the stored address is trimmed. Every in-tree writer trims in Rust
--    before it gets here, so this CHECK is unreachable through any shipped route; its job is
--    to make a FUTURE writer that forgets fail LOUDLY at the insert, rather than silently
--    reintroducing #251 through a leading/trailing `U+00A0` variant. See the header for why
--    the character set is enumerated and how the enumeration is kept honest.
ALTER TABLE public.users
    ADD CONSTRAINT users_email_trimmed CHECK (email = btrim(email, E'\u0009\u000A\u000B\u000C\u000D\u0020\u0085\u00A0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200A\u2028\u2029\u202F\u205F\u3000'));
