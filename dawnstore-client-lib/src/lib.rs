pub use dawnstore_lib::*;
use reqwest::{Client, Method, RequestBuilder};
use serde::{Serialize, de::DeserializeOwned};

#[derive(thiserror::Error, Debug)]
pub enum DawnstoreApiError {
    #[error("Error from reqwest: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("Error from api code: {0} msg: {1}")]
    ApiError(reqwest::StatusCode, String),
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
        Self { base_url, client: Client::new(), token }
    }

    pub fn get_client(&self) -> &Client {
        &self.client
    }

    pub fn get_base_url(&self) -> &str {
        &self.base_url
    }

    /// Build a request, attaching a Bearer token when one is configured.
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
            .request(Method::POST, format!("{}/get-resource-definitions", self.base_url))
            .json(filter)
            .send()
            .await?;
        to_result(resp).await
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
        to_result(resp).await
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
        to_result(resp).await
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
        to_result(resp).await
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
        to_result(resp).await
    }

    pub async fn delete_object(&self, req: &DeleteObject) -> Result<(), DawnstoreApiError> {
        let resp = self
            .request(Method::DELETE, format!("{}/delete-object", self.base_url))
            .json(req)
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(DawnstoreApiError::ApiError(resp.status(), resp.text().await?))
        }
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
        to_result(resp).await
    }
}

async fn to_result<T: DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, DawnstoreApiError> {
    if resp.status().is_success() {
        Ok(resp.json::<T>().await?)
    } else {
        Err(DawnstoreApiError::ApiError(resp.status(), resp.text().await?))
    }
}
