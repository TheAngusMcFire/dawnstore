-- Add migration script here
-- Enum for ForeignKeyType
CREATE TYPE foreign_key_type AS ENUM (
    'One',
    'OneOptional',
    'OneOrMany',
    'NoneOrMany'
);

-- Enum for ForeignKeyBehaviour
CREATE TYPE foreign_key_behaviour AS ENUM (
    'Fill',
    'Ignore'
);

CREATE TABLE foreign_key_constraints (
    id UUID PRIMARY KEY,
    api_version TEXT NOT NULL,
    kind TEXT NOT NULL,
    key_path TEXT NOT NULL,
    parent_key_path TEXT,
    type foreign_key_type NOT NULL,
    behaviour foreign_key_behaviour NOT NULL,
    foreign_key_kind TEXT
);

CREATE UNIQUE INDEX idx_foreign_key_constraints_lookup ON foreign_key_constraints (api_version, kind, key_path);

CREATE TABLE object_schemas (
    id UUID PRIMARY KEY,
    api_version TEXT NOT NULL,
    kind TEXT NOT NULL,
    aliases TEXT[] NOT NULL DEFAULT '{}',
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
    namespace TEXT NOT NULL,
    annotations JSONB NOT NULL DEFAULT '{}'::jsonb,
    labels JSONB NOT NULL DEFAULT '{}'::jsonb,
    spec JSONB NOT NULL
);

CREATE INDEX idx_objects_string_id_lookup ON objects (string_id);
CREATE INDEX idx_objects_lookup ON objects (namespace, kind, name);
CREATE INDEX idx_objects_labels ON objects USING GIN (labels);

CREATE TABLE relations (
    object_id UUID NOT NULL,
    foreign_object_id UUID NOT NULL,
    foreign_key_id UUID NOT NULL,
    PRIMARY KEY (object_id, foreign_object_id, foreign_key_id)
);

CREATE INDEX idx_relations_object_id ON relations (object_id);
CREATE INDEX idx_relations_foreign_object_id ON relations (foreign_object_id);
CREATE INDEX idx_relations_foreign_key_id ON relations (foreign_key_id);
