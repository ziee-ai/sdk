-- =====================================================
-- Migration 46: Pending account links + oauth_sessions.return_to
-- =====================================================
-- Supports the First-Broker-Login flow for social login (Keycloak
-- pattern). When a social-login email collides with an existing
-- local-password account we DO NOT auto-link — instead we mint a
-- single-use link_token, store the unconfirmed link here, and bounce
-- the user to /auth/link-account where they prove ownership by
-- entering the local password.
--
-- The return_to column on oauth_sessions lets the authorize endpoint
-- preserve the SPA's intended post-login URL without round-tripping
-- it through the provider's authorize URL (which would expose it to
-- the provider and to anyone with a Referer-header view).

CREATE TABLE pending_account_links (
    link_token VARCHAR(255) PRIMARY KEY,
    provider_id UUID NOT NULL REFERENCES auth_providers(id) ON DELETE CASCADE,
    target_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    external_id VARCHAR(255) NOT NULL,
    external_email VARCHAR(255),
    external_data JSONB,
    -- Per-token password-attempt counter. The global rate limit caps
    -- requests-per-IP, but a botnet with diverse IPs could still
    -- brute-force the local password during the 10-minute TTL.
    -- link_account increments this on every attempt and refuses
    -- further attempts past a hard cap (see handler for the value).
    -- Cheaper than a separate audit table and consistent with the
    -- token's single-use lifecycle.
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL
);

CREATE INDEX idx_pending_links_expires_at ON pending_account_links(expires_at);
CREATE INDEX idx_pending_links_target_user_id ON pending_account_links(target_user_id);

ALTER TABLE oauth_sessions ADD COLUMN return_to TEXT;
