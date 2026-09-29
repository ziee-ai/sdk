-- The session record: one row per signed-in session, and the home of that
-- session's access-token revocation epoch.
--
-- Before this, the epoch lived only on `users.token_version` (202607144600),
-- so every route-gating request read the identity table to check it, and the
-- only revocation a per-user scalar can express is "every session of this
-- user". The session row gives the epoch its standard home (Kratos `sessions`,
-- Medplum `Login`): the access token names its session in the `sid` claim, and
-- the extractors compare the token's `ver` against THIS row's `ver`.
--
--   * `ver`      — the session's epoch. Initialised from `users.token_version`
--                  at sign-in (the one place the user scalar is read); bumped
--                  alone to kill the session's tokens while the session
--                  survives (re-verification, idle-lock).
--   * `ended_at` — set when the session ends (logout of this session, or the
--                  user-level kill in `end_session_atomically`). An ended or
--                  absent row refuses the token 401 SESSION_REVOKED.
--
-- `users.token_version` stays the write-side master: logout still bumps it (in
-- the same transaction that ends the user's sessions), and a token with no
-- `sid` (minted before this migration) is still checked against it, so
-- deploying forces zero logouts.
CREATE TABLE IF NOT EXISTS public.auth_sessions (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    ver integer NOT NULL DEFAULT 0,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    ended_at timestamp with time zone
);

-- The user-level kill ends every live session of one user.
CREATE INDEX IF NOT EXISTS auth_sessions_user_live_idx
    ON public.auth_sessions (user_id)
    WHERE ended_at IS NULL;

COMMENT ON TABLE public.auth_sessions IS
    'One row per signed-in session. The access token names it in the `sid` claim; `ver` is the session''s revocation epoch (a request whose `ver` != this value, or whose row is absent/ended, is rejected 401 SESSION_REVOKED).';

-- The session dimension on the refresh-token whitelist: every refresh row of a
-- session's rotation family carries the session id. NULL = a row registered
-- before this migration (or through the legacy `register`), still fully usable.
ALTER TABLE public.refresh_tokens
    ADD COLUMN IF NOT EXISTS session_id uuid
        REFERENCES public.auth_sessions(id) ON DELETE CASCADE;

CREATE INDEX IF NOT EXISTS refresh_tokens_session_idx
    ON public.refresh_tokens (session_id)
    WHERE session_id IS NOT NULL;

COMMENT ON COLUMN public.refresh_tokens.session_id IS
    'The session (auth_sessions.id) this refresh token belongs to; inherited by every rotation successor. NULL for legacy rows.';
