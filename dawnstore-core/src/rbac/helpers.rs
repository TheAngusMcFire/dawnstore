/// Build the canonical `namespace/kind/name` string ID for a dawnstore object.
pub fn object_string_id(namespace: &str, kind: &str, name: &str) -> String {
    format!("{namespace}/{kind}/{name}")
}
