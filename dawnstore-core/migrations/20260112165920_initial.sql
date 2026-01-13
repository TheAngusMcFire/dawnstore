-- Add migration script here
CREATE TABLE foreign_key_constraints (
    id UUID PRIMARY KEY,
    api_version TEXT NOT NULL,
    kind TEXT NOT NULL,
    key_path TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_foreign_key_constraints_lookup ON foreign_key_constraints (api_version, kind);

CREATE TABLE object_schemas (
    id UUID PRIMARY KEY,
    api_version TEXT NOT NULL,
    kind TEXT NOT NULL,
    json_schema TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_object_schemas_lookup ON object_schemas (api_version, kind);

CREATE TABLE objects (
    id UUID PRIMARY KEY,
    string_id TEXT NOT NULL,
    api_version TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    namespace TEXT,
    annotations JSONB NOT NULL DEFAULT '{}'::jsonb,
    labels JSONB NOT NULL DEFAULT '{}'::jsonb,
    owners UUID[] NOT NULL DEFAULT '{}',
    spec JSONB NOT NULL
);

CREATE INDEX idx_objects_string_id_lookup ON objects (string_id);
CREATE INDEX idx_objects_lookup ON objects (namespace, kind, name);
CREATE INDEX idx_objects_labels ON objects USING GIN (labels);
