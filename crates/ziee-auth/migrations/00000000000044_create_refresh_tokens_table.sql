-- Refresh-token tracking for revocation + rotation. Closes 01-auth F-02
-- and F-03 (High): logout was a no-op (no server-side revocation), and
-- refresh issued new tokens without invalidating the presented one
-- (the old refresh token kept minting access tokens for up to 30 days).
--
-- Model: whitelist + soft-delete.
--   - On token issuance (login / register / oauth callback / refresh):
--     INSERT a row with the token's jti.
--   - On validation: row must exist with revoked_at IS NULL and
--     expires_at > NOW().
--   - On refresh: UPDATE old row SET revoked_at; INSERT new row.
--   - On logout: UPDATE all rows for the user SET revoked_at (sign
--     out everywhere).

CREATE TABLE refresh_tokens (
    jti UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    issued_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
    revoked_at TIMESTAMP WITH TIME ZONE
);

-- Partial index for the common "active for user" lookup. Closes 01-auth F-02 / F-03.
CREATE INDEX idx_refresh_tokens_user_active
    ON refresh_tokens(user_id)
    WHERE revoked_at IS NULL;

-- For cleanup jobs that purge expired rows
CREATE INDEX idx_refresh_tokens_expires_at ON refresh_tokens(expires_at);
