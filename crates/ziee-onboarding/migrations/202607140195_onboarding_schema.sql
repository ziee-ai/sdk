-- Squashed module baseline (MIGRATE-squash / N3.1 / N7).
-- onboarding module tables + indexes + triggers.

CREATE TABLE public.user_onboarding (
    user_id uuid NOT NULL,
    completed_guide_ids text[] DEFAULT '{}'::text[] NOT NULL,
    completed_step_ids text[] DEFAULT '{}'::text[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

ALTER TABLE ONLY public.user_onboarding
    ADD CONSTRAINT user_onboarding_pkey PRIMARY KEY (user_id);
