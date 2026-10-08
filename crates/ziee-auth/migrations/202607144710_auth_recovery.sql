-- Self-service account recovery for accounts with no email.
--
-- Three tables, all owned by ziee-auth, all keyed to `users` and removed with
-- the user. Nothing in them is ever returned by an endpoint: the codes and
-- answers are stored only as bcrypt hashes.

-- One row per recovery code. A "set" is every row sharing a batch_id; a
-- regeneration deletes the previous rows in the same transaction as the
-- insert of the new ones, so there is never more than one live set.
CREATE TABLE public.auth_recovery_codes (
    id uuid DEFAULT gen_random_uuid() NOT NULL PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    batch_id uuid NOT NULL,
    code_hash character varying(255) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    used_at timestamp with time zone
);
CREATE INDEX idx_auth_recovery_codes_user ON public.auth_recovery_codes (user_id) WHERE used_at IS NULL;

-- One row per configured security question (2 or 3 per user, enforced by the
-- repository inside one transaction). `question_key` names an entry in the
-- fixed catalogue compiled into the crate; there is no free-text question.
CREATE TABLE public.auth_security_questions (
    user_id uuid NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    position smallint NOT NULL CHECK (position BETWEEN 1 AND 3),
    question_key character varying(64) NOT NULL,
    answer_hash character varying(255) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    PRIMARY KEY (user_id, position),
    UNIQUE (user_id, question_key)
);

-- Failure counters for the reset and re-authentication endpoints. `scope` is
-- 'name' (the submitted username, lowercased, whether or not it exists),
-- 'ip' or 'reauth' (a user id). Keyed by what the CALLER supplied, never by
-- whether the account exists, so a lockout cannot be used to probe for names.
-- No FK on purpose: a name that matches no user must be counted too.
CREATE TABLE public.auth_recovery_attempts (
    scope character varying(16) NOT NULL,
    key character varying(255) NOT NULL,
    failures integer DEFAULT 0 NOT NULL,
    window_started_at timestamp with time zone DEFAULT now() NOT NULL,
    locked_until timestamp with time zone,
    PRIMARY KEY (scope, key)
);
CREATE INDEX idx_auth_recovery_attempts_window ON public.auth_recovery_attempts (window_started_at);
