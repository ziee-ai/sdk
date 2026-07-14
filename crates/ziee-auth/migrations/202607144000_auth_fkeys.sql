-- Squashed module baseline (MIGRATE-squash / N3.1 / N7).
-- Auth foreign keys (deferred).

ALTER TABLE ONLY public.oauth_sessions
    ADD CONSTRAINT oauth_sessions_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.auth_providers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.pending_account_links
    ADD CONSTRAINT pending_account_links_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.auth_providers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.pending_account_links
    ADD CONSTRAINT pending_account_links_target_user_id_fkey FOREIGN KEY (target_user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_auth_links
    ADD CONSTRAINT user_auth_links_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.auth_providers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_auth_links
    ADD CONSTRAINT user_auth_links_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_groups
    ADD CONSTRAINT user_groups_assigned_by_fkey FOREIGN KEY (assigned_by) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.user_groups
    ADD CONSTRAINT user_groups_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.groups(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_groups
    ADD CONSTRAINT user_groups_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
