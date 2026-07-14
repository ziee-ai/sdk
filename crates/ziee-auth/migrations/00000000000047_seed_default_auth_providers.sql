-- =====================================================
-- Migration 47: Pre-seed Google / Microsoft / Apple as disabled
-- =====================================================
-- Discoverability: instead of starting with an empty table and
-- forcing the admin to find the "Add provider" dropdown, surface
-- the three common providers as disabled rows with sensible defaults
-- pre-filled. The admin's natural flow becomes:
--   1. Open /settings/auth-providers
--   2. See Google / Microsoft / Apple listed
--   3. Click Edit on the one they want
--   4. Paste client_id + client_secret (or, for Apple, team_id
--      + services_id + key_id + .p8 path)
--   5. Click "Test config" (verify) → Save → toggle Enabled
--   6. "Sign in with X" button appears on /login
--
-- All three start with enabled=false. The public
-- `/api/auth/providers` endpoint already filters `enabled = true`,
-- so they don't appear on the login page until configured.

INSERT INTO auth_providers (name, provider_type, enabled, config) VALUES
(
    'google',
    'oidc',
    false,
    jsonb_build_object(
        'client_id', '',
        'client_secret', '',
        'issuer_url', 'https://accounts.google.com',
        'scopes', jsonb_build_array('openid', 'email', 'profile'),
        'attribute_mapping', jsonb_build_object(
            'user_id', 'sub',
            'username', 'email',
            'email', 'email',
            'display_name', 'name',
            'first_name', 'given_name',
            'last_name', 'family_name'
        ),
        'display_name', 'Sign in with Google'
    )
),
(
    'microsoft',
    'oidc',
    false,
    jsonb_build_object(
        'client_id', '',
        'client_secret', '',
        'issuer_url', 'https://login.microsoftonline.com/common/v2.0',
        'scopes', jsonb_build_array('openid', 'email', 'profile'),
        'attribute_mapping', jsonb_build_object(
            'user_id', 'sub',
            'username', 'preferred_username',
            'email', 'email',
            'display_name', 'name'
        ),
        'allowed_tenant_ids', jsonb_build_array(),
        'display_name', 'Sign in with Microsoft'
    )
),
(
    'apple',
    'apple',
    false,
    jsonb_build_object(
        'team_id', '',
        'services_id', '',
        'key_id', '',
        'private_key_path', '',
        'scopes', jsonb_build_array('name', 'email')
    )
);
