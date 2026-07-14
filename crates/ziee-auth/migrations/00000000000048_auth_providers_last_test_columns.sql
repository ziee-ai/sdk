-- =====================================================
-- Migration 48: Persist auth-provider test results
-- =====================================================
-- The Test button in the admin UI surfaces a connection-test
-- result. Previously this lived only in the frontend store and
-- was lost on reload. Persisting it on the row lets the UI render
-- "last tested ✓ 2m ago" / "last tested ✗ 1h ago" after any
-- refresh, and gives operators a stable record of when a provider
-- was last known good.

ALTER TABLE auth_providers
    ADD COLUMN last_test_at TIMESTAMP WITH TIME ZONE NULL,
    ADD COLUMN last_test_ok BOOLEAN NULL,
    ADD COLUMN last_test_message TEXT NULL;
