-- Generic, domain-agnostic notification inbox (SDK notification feature).
--
-- PROPOSED SDK CONTRIBUTION (see SDK_GAPS): the former ziee-chat-specific
-- FK-nullable columns (scheduled_task_id / workflow_run_id / conversation_id)
-- are replaced by ONE `payload jsonb` column, so a module contributes its own
-- notification `kind` + kind-specific payload without the SDK schema knowing any
-- domain. The inbox stays strictly per-user (every query is `WHERE user_id`).
-- No FK to `users` in the crate migration (an app that has a users table adds
-- the constraint in its own module migration) — applies standalone on a fresh DB.

CREATE TABLE public.notifications (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    kind text NOT NULL,
    title text NOT NULL,
    body text DEFAULT ''::text NOT NULL,
    interrupt boolean DEFAULT true NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    read_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_pkey PRIMARY KEY (id);

CREATE INDEX idx_notifications_user_created ON public.notifications USING btree (user_id, created_at DESC);

CREATE INDEX idx_notifications_user_unread ON public.notifications USING btree (user_id) WHERE (read_at IS NULL);
