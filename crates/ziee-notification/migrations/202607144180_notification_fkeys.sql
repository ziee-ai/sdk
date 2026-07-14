-- Squashed module baseline (MIGRATE-squash / N3.1 / N7).
-- notification foreign keys (deferred).

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_scheduled_task_id_fkey FOREIGN KEY (scheduled_task_id) REFERENCES public.scheduled_tasks(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_workflow_run_id_fkey FOREIGN KEY (workflow_run_id) REFERENCES public.workflow_runs(id) ON DELETE SET NULL;
