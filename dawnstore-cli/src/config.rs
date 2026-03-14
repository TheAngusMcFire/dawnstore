#[derive(serde::Deserialize)]
pub struct Context {
    pub url: String,
    /// Optional Bearer token. Can be overridden by `--token` / `DAWNSTORE_TOKEN`.
    pub token: Option<String>,
}
