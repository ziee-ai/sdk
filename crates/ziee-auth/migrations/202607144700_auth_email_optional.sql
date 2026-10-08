-- Trustless accounts: a user may have no email at all.
--
-- `users.email` was NOT NULL UNIQUE. A deployment that signs people up with a
-- username and password only (`auth.email_required: false`) stores NULL here.
-- The existing UNIQUE constraint already ignores NULLs, so uniqueness keeps its
-- meaning among real emails and any number of email-less accounts coexist.
-- An empty string must never be stored as "no email": two of them would
-- collide on the constraint, which is why every writer maps '' to NULL.
ALTER TABLE public.users ALTER COLUMN email DROP NOT NULL;
