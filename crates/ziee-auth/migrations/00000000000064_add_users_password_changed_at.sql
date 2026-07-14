-- Add password_changed_at to track whether the admin user has rotated
-- the bootstrap default password ("desktop-auto-login"). Used by the
-- Remote Access module to gate the "Enable password authentication"
-- toggle: if the admin's password is still the well-known bootstrap
-- value, we refuse to enable a public password-login surface.
--
-- NULL means "never changed" (still bootstrap default). Set to NOW()
-- on every password update path in the user module.

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS password_changed_at TIMESTAMPTZ NULL;

COMMENT ON COLUMN users.password_changed_at IS
    'Timestamp of the most recent password change. NULL means the user is still using their bootstrap-issued password. Remote Access password-auth toggle requires this to be non-NULL for the admin user.';
