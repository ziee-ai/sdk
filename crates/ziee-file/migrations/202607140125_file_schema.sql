-- Squashed module baseline (MIGRATE-squash / N3.1 / N7).
-- file module tables + indexes + triggers.

CREATE TABLE public.file_versions (
    id uuid NOT NULL,
    file_id uuid NOT NULL,
    version integer NOT NULL,
    is_head boolean DEFAULT false NOT NULL,
    blob_version_id uuid NOT NULL,
    file_size bigint NOT NULL,
    mime_type character varying(100),
    checksum character varying(64),
    has_thumbnail boolean DEFAULT false NOT NULL,
    preview_page_count integer DEFAULT 0 NOT NULL,
    text_page_count integer DEFAULT 0 NOT NULL,
    processing_metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    source_message_id uuid,
    created_by character varying(10) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.files (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    filename character varying(255) NOT NULL,
    file_size bigint NOT NULL,
    mime_type character varying(100),
    checksum character varying(64),
    has_thumbnail boolean DEFAULT false NOT NULL,
    preview_page_count integer DEFAULT 0 NOT NULL,
    text_page_count integer DEFAULT 0 NOT NULL,
    processing_metadata jsonb DEFAULT '{}'::jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    created_by character varying(10) DEFAULT 'user'::character varying NOT NULL,
    current_version_id uuid NOT NULL
);

ALTER TABLE ONLY public.file_versions
    ADD CONSTRAINT file_versions_file_id_version_key UNIQUE (file_id, version);

ALTER TABLE ONLY public.file_versions
    ADD CONSTRAINT file_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.files
    ADD CONSTRAINT files_pkey PRIMARY KEY (id);

CREATE INDEX idx_file_versions_blob ON public.file_versions USING btree (blob_version_id);

CREATE INDEX idx_file_versions_file ON public.file_versions USING btree (file_id, version DESC);

CREATE INDEX idx_files_checksum ON public.files USING btree (checksum);

CREATE INDEX idx_files_created_at ON public.files USING btree (created_at DESC);

CREATE INDEX idx_files_file_size ON public.files USING btree (file_size);

CREATE INDEX idx_files_mime_type ON public.files USING btree (mime_type);

CREATE INDEX idx_files_processing_metadata ON public.files USING gin (processing_metadata);

CREATE INDEX idx_files_user_id ON public.files USING btree (user_id);


CREATE UNIQUE INDEX uq_file_versions_head ON public.file_versions USING btree (file_id) WHERE is_head;
