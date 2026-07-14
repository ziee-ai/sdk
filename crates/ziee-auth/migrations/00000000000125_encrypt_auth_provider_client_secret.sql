-- Encrypt the auth-provider (login) OAuth client_secret at rest using the
-- existing dual-column pattern (mirrors migration 120's
-- mcp_server_oauth_configs.client_secret_encrypted, plus
-- web_search_providers.api_key and llm_repositories.auth_config_encrypted).
--
-- Until now the client_secret lived in PLAINTEXT inside the auth_providers.config
-- JSONB column (`config->>'client_secret'`). New/updated rows now store the
-- secret encrypted in client_secret_encrypted (pgcrypto pgp_sym_encrypt via
-- common::secret::encrypt_secret) and blank the plaintext copy inside config;
-- legacy rows whose config still carries the plaintext secret are resolved via
-- resolve_optional_secret's fallback until their next write re-encrypts them.
--
-- Only the client_secret field is encrypted here (the OAuth login secret). The
-- other sensitive config keys (bind_password / private_key_path) remain in
-- config and are out of scope for this change; the API response layer continues
-- to mask all of them.
ALTER TABLE auth_providers
    ADD COLUMN IF NOT EXISTS client_secret_encrypted BYTEA;
