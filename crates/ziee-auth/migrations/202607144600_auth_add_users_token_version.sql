-- Access-token revocation epoch.
--
-- Logout revokes every refresh token (the whitelist), but the ACCESS token is
-- a stateless bearer credential: signature + exp are the only things checked,
-- so it kept working for its full TTL (24h by default) after logout. A
-- held/exfiltrated token therefore survived the logout that was supposed to
-- end it.
--
-- This counter is the revocation epoch. It is stamped onto each access token
-- as the `ver` claim at mint time and compared for EQUALITY on every
-- authenticated request; logout bumps it, so every token minted before the
-- logout stops validating immediately.
--
-- Why a counter and not a `sessions_revoked_at` timestamp compared to the
-- token's `iat`: `iat` is whole seconds while NOW() has microseconds, so a
-- token minted just before a logout and one minted just after a re-login
-- within the SAME second are bit-identical in `iat` — no comparison can
-- separate them. The counter orders causally rather than temporally, so it has
-- no time-granularity failure mode.
--
-- DEFAULT 0 + an optional `ver` claim keeps already-issued tokens working
-- (absent claim => 0 => matches), so deploying this forces zero logouts.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS token_version INTEGER NOT NULL DEFAULT 0;

COMMENT ON COLUMN users.token_version IS
    'Access-token revocation epoch. Stamped on access tokens as the `ver` claim; bumped by logout (transactionally, together with revoking the user''s refresh tokens). A request whose `ver` != this value is rejected 401 SESSION_REVOKED. No index: only ever read by the users PK.';
