-- Squashed module baseline (MIGRATE-squash / N3.1 / N7).
-- Auth seed: system groups with CLEAN base permissions (N9 — domain perms
-- are granted by each owning module's data migration), default auth providers,
-- session settings singleton.

INSERT INTO public.groups VALUES ('f3a2b018-a0eb-45c1-98f9-a616599c9ee4', 'Administrators', 'System administrators with full access to all features', '{*}', true, true, false, '2026-07-14 15:09:55.356077+00', '2026-07-14 15:09:58.777409+00');
INSERT INTO public.groups VALUES ('79668b78-16ff-4d3f-80e6-afdbb3236dc9', 'Users', 'Default group for all users', '{profile::read,profile::edit}', true, true, true, '2026-07-14 15:09:55.356917+00', '2026-07-14 15:10:01.610531+00');

INSERT INTO public.auth_providers VALUES ('92e74a99-ffa4-4c49-a05f-4a2b3e2b2efe', 'google', 'oidc', false, '{"scopes": ["openid", "email", "profile"], "client_id": "", "issuer_url": "https://accounts.google.com", "display_name": "Sign in with Google", "client_secret": "", "attribute_mapping": {"email": "email", "user_id": "sub", "username": "email", "last_name": "family_name", "first_name": "given_name", "display_name": "name"}}', '2026-07-14 15:09:57.221494+00', '2026-07-14 15:09:57.221494+00', NULL, NULL, NULL, NULL);
INSERT INTO public.auth_providers VALUES ('824eb523-bda9-40dd-8a06-b71c5e1ced97', 'microsoft', 'oidc', false, '{"scopes": ["openid", "email", "profile"], "client_id": "", "issuer_url": "https://login.microsoftonline.com/common/v2.0", "display_name": "Sign in with Microsoft", "client_secret": "", "attribute_mapping": {"email": "email", "user_id": "sub", "username": "preferred_username", "display_name": "name"}, "allowed_tenant_ids": []}', '2026-07-14 15:09:57.221494+00', '2026-07-14 15:09:57.221494+00', NULL, NULL, NULL, NULL);
INSERT INTO public.auth_providers VALUES ('a159dee0-2977-4f8d-a8e5-68dc1a398c32', 'apple', 'apple', false, '{"key_id": "", "scopes": ["name", "email"], "team_id": "", "services_id": "", "private_key_path": ""}', '2026-07-14 15:09:57.221494+00', '2026-07-14 15:09:57.221494+00', NULL, NULL, NULL, NULL);

INSERT INTO public.session_settings VALUES (true, 24, 30, false, '2026-07-14 15:10:00.732948+00');
