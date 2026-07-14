-- Session settings (auth module) + refresh-token rotation grace.
--
-- 1. `session_settings` — singleton deployment-wide config for JWT
--    lifetimes: the access-token TTL (hours) and the max session length
--    (the refresh-token TTL, days). Both were previously static YAML
--    (`jwt.access_token_expiry_hours` / `jwt.refresh_token_expiry_days`);
--    the YAML values remain the *initial seed* (copied in once at boot by
--    the auth module — see `seeded_from_config`) and the mint-time
--    fallback if the DB read fails. Singleton enforced via
--    `id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id = TRUE)` (mirrors
--    web_search_settings, migration 97).
--
-- 2. `refresh_tokens.rotated_to` — rotation-grace support. Refresh-token
--    rotation is single-use: the presented jti is revoked the moment a new
--    pair is minted. Two devices/tabs refreshing concurrently would log
--    the loser out. Rotation now records the successor jti in
--    `rotated_to`; a refresh presenting a jti revoked-by-rotation within
--    the last 30 seconds is treated as the racing legitimate client and
--    served a fresh pair. Logout/password-change revocation leaves
--    `rotated_to` NULL, so revoked-for-real tokens always hard-fail.

CREATE TABLE session_settings (
    id                          BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id = TRUE),

    -- Access-token TTL. Short is safer (a disabled user is cut off at the
    -- next refresh); the client silently refreshes before expiry so active
    -- sessions are unaffected.
    access_token_expiry_hours   INTEGER NOT NULL DEFAULT 24,

    -- Max session length: how long a session survives with NO activity.
    -- Active sessions roll (each refresh mints a new refresh token with a
    -- fresh TTL), so this is the idle bound, not an activity cap.
    refresh_token_expiry_days   INTEGER NOT NULL DEFAULT 30,

    -- Set TRUE after the one-time boot copy of the YAML jwt values into
    -- this row. Thereafter the DB is authoritative and YAML edits are
    -- ignored (documented in config examples).
    seeded_from_config          BOOLEAN NOT NULL DEFAULT FALSE,

    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Defense-in-depth range guards; the handler validates first for
    -- clearer errors, the DB is the last line.
    CONSTRAINT access_token_expiry_hours_range
        CHECK (access_token_expiry_hours BETWEEN 1 AND 8760),   -- 1 hour .. 1 year
    CONSTRAINT refresh_token_expiry_days_range
        CHECK (refresh_token_expiry_days BETWEEN 1 AND 3650)    -- 1 day .. 10 years
);

COMMENT ON TABLE session_settings IS
    'Singleton deployment-wide JWT session config (access-token TTL + max session length).';

INSERT INTO session_settings (id) VALUES (TRUE)
ON CONFLICT (id) DO NOTHING;

-- Rotation grace: the successor jti minted when this token was rotated.
-- NULL for tokens revoked by logout / password change / admin action —
-- only rotation sets it, and only rotation-revoked tokens get the grace.
ALTER TABLE refresh_tokens ADD COLUMN rotated_to UUID;

COMMENT ON COLUMN refresh_tokens.rotated_to IS
    'Successor jti when revoked by rotation (30s grace for racing clients); NULL when revoked by logout.';
