-- Squashed module baseline (MIGRATE-squash / N3.1 / N7).
-- Auth structural tables (ziee-auth owns identity schema).

-- Shared trigger fn (also defined by the framework bootstrap; repeated
-- here idempotently so the auth-only migrator is self-contained).
CREATE OR REPLACE FUNCTION update_updated_at_column() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE TABLE public.auth_providers (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name character varying(100) NOT NULL,
    provider_type character varying(50) NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    config jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    last_test_at timestamp with time zone,
    last_test_ok boolean,
    last_test_message text,
    client_secret_encrypted bytea
);

CREATE TABLE public.groups (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name character varying(100) NOT NULL,
    description text,
    permissions text[] DEFAULT '{}'::text[] NOT NULL,
    is_system boolean DEFAULT false NOT NULL,
    is_active boolean DEFAULT true NOT NULL,
    is_default boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.oauth_sessions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    state character varying(255) NOT NULL,
    provider_id uuid NOT NULL,
    pkce_verifier character varying(255),
    nonce character varying(255),
    redirect_uri text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    return_to text
);

CREATE TABLE public.pending_account_links (
    link_token character varying(255) NOT NULL,
    provider_id uuid NOT NULL,
    target_user_id uuid NOT NULL,
    external_id character varying(255) NOT NULL,
    external_email character varying(255),
    external_data jsonb,
    attempts integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

CREATE TABLE public.refresh_tokens (
    jti uuid NOT NULL,
    user_id uuid NOT NULL,
    issued_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    rotated_to uuid
);

CREATE TABLE public.session_settings (
    id boolean DEFAULT true NOT NULL,
    access_token_expiry_hours integer DEFAULT 24 NOT NULL,
    refresh_token_expiry_days integer DEFAULT 30 NOT NULL,
    seeded_from_config boolean DEFAULT false NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT access_token_expiry_hours_range CHECK (((access_token_expiry_hours >= 1) AND (access_token_expiry_hours <= 8760))),
    CONSTRAINT refresh_token_expiry_days_range CHECK (((refresh_token_expiry_days >= 1) AND (refresh_token_expiry_days <= 3650))),
    CONSTRAINT session_settings_id_check CHECK ((id = true))
);

CREATE TABLE public.user_auth_links (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    provider_id uuid NOT NULL,
    external_id character varying(255) NOT NULL,
    external_email character varying(255),
    external_data jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    last_login_at timestamp with time zone
);

CREATE TABLE public.user_groups (
    user_id uuid NOT NULL,
    group_id uuid NOT NULL,
    assigned_at timestamp with time zone DEFAULT now() NOT NULL,
    assigned_by uuid
);

CREATE TABLE public.users (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    username character varying(100) NOT NULL,
    email character varying(255) NOT NULL,
    email_verified boolean DEFAULT false NOT NULL,
    password_hash character varying(255),
    display_name character varying(255),
    avatar_url text,
    is_active boolean DEFAULT true NOT NULL,
    is_admin boolean DEFAULT false NOT NULL,
    permissions text[] DEFAULT '{}'::text[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    last_login_at timestamp with time zone,
    password_changed_at timestamp with time zone
);

ALTER TABLE ONLY public.auth_providers
    ADD CONSTRAINT auth_providers_name_key UNIQUE (name);

ALTER TABLE ONLY public.auth_providers
    ADD CONSTRAINT auth_providers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.groups
    ADD CONSTRAINT groups_name_key UNIQUE (name);

ALTER TABLE ONLY public.groups
    ADD CONSTRAINT groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.oauth_sessions
    ADD CONSTRAINT oauth_sessions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.oauth_sessions
    ADD CONSTRAINT oauth_sessions_state_key UNIQUE (state);

ALTER TABLE ONLY public.pending_account_links
    ADD CONSTRAINT pending_account_links_pkey PRIMARY KEY (link_token);

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_pkey PRIMARY KEY (jti);

ALTER TABLE ONLY public.session_settings
    ADD CONSTRAINT session_settings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.user_auth_links
    ADD CONSTRAINT user_auth_links_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.user_auth_links
    ADD CONSTRAINT user_auth_links_provider_id_external_id_key UNIQUE (provider_id, external_id);

ALTER TABLE ONLY public.user_groups
    ADD CONSTRAINT user_groups_pkey PRIMARY KEY (user_id, group_id);

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_email_key UNIQUE (email);

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_username_key UNIQUE (username);

CREATE INDEX idx_auth_providers_enabled ON public.auth_providers USING btree (enabled);

CREATE INDEX idx_groups_name ON public.groups USING btree (name);

CREATE INDEX idx_groups_permissions ON public.groups USING gin (permissions);

CREATE INDEX idx_oauth_sessions_expires_at ON public.oauth_sessions USING btree (expires_at);

CREATE INDEX idx_oauth_sessions_state ON public.oauth_sessions USING btree (state);

CREATE INDEX idx_pending_links_expires_at ON public.pending_account_links USING btree (expires_at);

CREATE INDEX idx_pending_links_target_user_id ON public.pending_account_links USING btree (target_user_id);

CREATE INDEX idx_refresh_tokens_expires_at ON public.refresh_tokens USING btree (expires_at);

CREATE INDEX idx_refresh_tokens_user_active ON public.refresh_tokens USING btree (user_id) WHERE (revoked_at IS NULL);

CREATE INDEX idx_user_auth_links_external_id ON public.user_auth_links USING btree (provider_id, external_id);

CREATE INDEX idx_user_auth_links_provider_id ON public.user_auth_links USING btree (provider_id);

CREATE INDEX idx_user_auth_links_user_id ON public.user_auth_links USING btree (user_id);

CREATE INDEX idx_user_groups_group_id ON public.user_groups USING btree (group_id);

CREATE INDEX idx_user_groups_user_id ON public.user_groups USING btree (user_id);

CREATE INDEX idx_users_created_at ON public.users USING btree (created_at);

CREATE INDEX idx_users_email ON public.users USING btree (email);

CREATE INDEX idx_users_is_active ON public.users USING btree (is_active);

CREATE INDEX idx_users_last_login_at ON public.users USING btree (last_login_at);

CREATE INDEX idx_users_lower_email ON public.users USING btree (lower((email)::text));

CREATE INDEX idx_users_permissions ON public.users USING gin (permissions);

CREATE INDEX idx_users_username ON public.users USING btree (username);

CREATE UNIQUE INDEX unique_root_admin ON public.users USING btree (is_admin) WHERE (is_admin = true);

CREATE TRIGGER update_auth_providers_updated_at BEFORE UPDATE ON public.auth_providers FOR EACH ROW EXECUTE FUNCTION public.update_updated_at_column();

CREATE TRIGGER update_groups_updated_at BEFORE UPDATE ON public.groups FOR EACH ROW EXECUTE FUNCTION public.update_updated_at_column();

CREATE TRIGGER update_user_auth_links_updated_at BEFORE UPDATE ON public.user_auth_links FOR EACH ROW EXECUTE FUNCTION public.update_updated_at_column();

CREATE TRIGGER update_users_updated_at BEFORE UPDATE ON public.users FOR EACH ROW EXECUTE FUNCTION public.update_updated_at_column();
