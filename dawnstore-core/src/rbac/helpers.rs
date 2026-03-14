/// Build the canonical `namespace/kind/name` string ID for a dawnstore object.
pub fn object_string_id(namespace: &str, kind: &str, name: &str) -> String {
    format!("{namespace}/{kind}/{name}")
}

/// Build the cache key used to index schemas and FK constraints: `api_version/kind`.
pub fn schema_cache_key(api_version: &str, kind: &str) -> String {
    format!("{api_version}/{kind}")
}
