CREATE SCHEMA IF NOT EXISTS promptkit;

CREATE TABLE promptkit.users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    profile JSONB,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE promptkit.access_tokens (
    token BYTEA PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES promptkit.users(id) ON DELETE CASCADE,
    comment TEXT,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMP WITH TIME ZONE
);

CREATE TYPE promptkit.function_visibility AS ENUM('public', 'internal', 'private');
CREATE TYPE promptkit.function_permission AS ENUM('owner', 'editor', 'viewer');

CREATE TABLE promptkit.functions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    endpoint TEXT UNIQUE,
    description JSONB,
    visibility promptkit.function_visibility NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE promptkit.functions_users (
    function_id UUID NOT NULL REFERENCES promptkit.functions(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES promptkit.users(id) ON DELETE CASCADE,
    permission promptkit.function_permission NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (function_id, user_id)
);

CREATE TABLE promptkit.revisions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    function_id UUID NOT NULL REFERENCES promptkit.functions(id) ON DELETE CASCADE,
    runtime JSONB NOT NULL,
    storage_key TEXT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    published_at TIMESTAMP WITH TIME ZONE
);

ALTER TABLE promptkit.functions ADD COLUMN main_revision_id UUID REFERENCES promptkit.revisions(id) ON DELETE SET NULL;