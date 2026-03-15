pub use dawnstore_lib::*;

mod context;
pub use context::Context;
use reqwest::{Client, Method, RequestBuilder};
use serde::{Serialize, de::DeserializeOwned};

#[derive(thiserror::Error, Debug)]
pub enum DawnstoreApiError {
    #[error("Transport error: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("HTTP error {0}: {1}")]
    HttpError(reqwest::StatusCode, String),
    #[error("Server error: {0:?}")]
    ServerError(DawnStoreApiError),
}

pub struct Api {
    base_url: String,
    client: Client,
    token: Option<String>,
}

impl Api {
    pub fn new(url: impl Into<String>) -> Self {
        Self::build(url, None)
    }

    pub fn new_with_token(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self::build(url, Some(token.into()))
    }

    fn build(url: impl Into<String>, token: Option<String>) -> Self {
        let base_url = url.into();
        if base_url.ends_with("/") {
            panic!("url can not end with /");
        }
        Self {
            base_url,
            client: Client::new(),
            token,
        }
    }

    pub fn get_client(&self) -> &Client {
        &self.client
    }

    pub fn get_base_url(&self) -> &str {
        &self.base_url
    }

    fn request(&self, method: Method, url: String) -> RequestBuilder {
        let builder = self.client.request(method, url);
        match &self.token {
            Some(t) => builder.bearer_auth(t),
            None => builder,
        }
    }

    pub async fn get_resource_definitions(
        &self,
        filter: &GetResourceDefinitionFilter,
    ) -> Result<Vec<ResourceDefinition>, DawnstoreApiError> {
        let resp = self
            .request(
                Method::POST,
                format!("{}/get-resource-definitions", self.base_url),
            )
            .json(filter)
            .send()
            .await?;
        envelope(resp).await
    }

    pub async fn get_objects(
        &self,
        filter: &GetObjectsFilter,
    ) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnstoreApiError> {
        let resp = self
            .request(Method::POST, format!("{}/get-objects", self.base_url))
            .json(filter)
            .send()
            .await?;
        envelope(resp).await
    }

    pub async fn get_objects_typed<T: DeserializeOwned>(
        &self,
        filter: &GetObjectsFilter,
    ) -> Result<Vec<ReturnObject<T>>, DawnstoreApiError> {
        let resp = self
            .request(Method::POST, format!("{}/get-objects", self.base_url))
            .json(filter)
            .send()
            .await?;
        envelope(resp).await
    }

    pub async fn get_object_infos(
        &self,
        filter: &GetObjectInfosFilter,
    ) -> Result<ObjectInfos, DawnstoreApiError> {
        let resp = self
            .request(Method::POST, format!("{}/get-object-infos", self.base_url))
            .json(filter)
            .send()
            .await?;
        envelope(resp).await
    }

    pub async fn issue_service_account_token(
        &self,
        req: &IssueTokenRequest,
    ) -> Result<IssueTokenResponse, DawnstoreApiError> {
        let resp = self
            .request(Method::POST, format!("{}/rbac/issue-token", self.base_url))
            .json(req)
            .send()
            .await?;
        envelope(resp).await
    }

    pub async fn apply<T: Serialize>(
        &self,
        obj: &Object<T>,
    ) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnstoreApiError> {
        let resp = self
            .request(Method::POST, format!("{}/apply", self.base_url))
            .json(obj)
            .send()
            .await?;
        envelope(resp).await
    }

    pub async fn apply_str(
        &self,
        content: String,
    ) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnstoreApiError> {
        let resp = self
            .request(Method::POST, format!("{}/apply", self.base_url))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(content)
            .send()
            .await?;
        envelope(resp).await
    }

    pub async fn delete_object(&self, req: &DeleteObject) -> Result<(), DawnstoreApiError> {
        let resp = self
            .request(Method::DELETE, format!("{}/delete-object", self.base_url))
            .json(req)
            .send()
            .await?;
        let _: serde_json::Value = envelope(resp).await?;
        Ok(())
    }

    pub async fn reqwest_exchange<Treq: Serialize, Tres: DeserializeOwned>(
        &self,
        url: impl FnOnce(&str) -> String,
        req: &Treq,
    ) -> Result<Tres, DawnstoreApiError> {
        let resp = self
            .request(Method::POST, url(self.get_base_url()))
            .json(req)
            .send()
            .await?;
        envelope(resp).await
    }
}

/// Unwrap a `DawnStoreResponse<T>` from a response.
/// Non-200 HTTP status codes (e.g. 401 Unauthorized) are returned as
/// [`DawnstoreApiError::HttpError`] without attempting JSON parsing.
/// 200 responses are parsed as the envelope and the inner error (if any)
/// is returned as [`DawnstoreApiError::ServerError`].
async fn envelope<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T, DawnstoreApiError> {
    if !resp.status().is_success() {
        return Err(DawnstoreApiError::HttpError(
            resp.status(),
            resp.text().await?,
        ));
    }
    let wrapped: DawnStoreResponse<T> = resp.json().await?;
    match wrapped {
        DawnStoreResponse {
            data: Some(data), ..
        } => Ok(data),
        DawnStoreResponse {
            error: Some(err), ..
        } => Err(DawnstoreApiError::ServerError(err)),
        _ => Err(DawnstoreApiError::HttpError(
            reqwest::StatusCode::OK,
            "server returned an empty response".to_string(),
        )),
    }
}
