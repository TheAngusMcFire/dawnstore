pub use dawnstore_lib::*;
use reqwest::{Client, IntoUrl};
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
    client: reqwest::Client,
}

impl Api {
    pub fn new(url: impl Into<String>) -> Self {
        let base_url = url.into();
        if base_url.ends_with("/") {
            panic!("url can not end with /");
        }
        Self {
            base_url,
            client: Client::new(),
        }
    }

    pub fn get_client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn get_base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn get_resource_definitions(
        &self,
        filter: &GetResourceDefinitionFilter,
    ) -> Result<Vec<ResourceDefinition>, DawnstoreApiError> {
        let i = self
            .client
            .post(format!("{}/get-resource-definitions", self.base_url))
            .json(filter)
            .send()
            .await?;
        if i.status().is_success() {
            Ok(i.json::<Vec<ResourceDefinition>>().await?)
        } else {
            Err(DawnstoreApiError::ApiError(i.status(), i.text().await?))
        }
    }

    pub async fn get_objects(
        &self,
        filter: &GetObjectsFilter,
    ) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnstoreApiError> {
        let i = self
            .client
            .post(format!("{}/get-objects", self.base_url))
            .json(filter)
            .send()
            .await?;
        if i.status().is_success() {
            Ok(i.json::<Vec<ReturnObject<serde_json::Value>>>().await?)
        } else {
            Err(DawnstoreApiError::ApiError(i.status(), i.text().await?))
        }
    }

    pub async fn get_object_infos(
        &self,
        filter: &GetObjectInfosFilter,
    ) -> Result<ObjectInfos, DawnstoreApiError> {
        let i = self
            .client
            .post(format!("{}/get-object-infos", self.base_url))
            .json(filter)
            .send()
            .await?;
        if i.status().is_success() {
            Ok(i.json::<ObjectInfos>().await?)
        } else {
            Err(DawnstoreApiError::ApiError(i.status(), i.text().await?))
        }
    }

    pub async fn apply_str(
        &self,
        content: String,
    ) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnstoreApiError> {
        let i = self
            .client
            .post(format!("{}/apply", self.base_url))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(content)
            .send()
            .await?;
        if i.status().is_success() {
            Ok(i.json::<Vec<ReturnObject<serde_json::Value>>>().await?)
        } else {
            Err(DawnstoreApiError::ApiError(i.status(), i.text().await?))
        }
    }

    pub async fn delete_object(&self, req: &DeleteObject) -> Result<(), DawnstoreApiError> {
        let i = self
            .client
            .delete(format!("{}/delete-object", self.base_url))
            .json(req)
            .send()
            .await?;
        if i.status().is_success() {
            Ok(())
        } else {
            Err(DawnstoreApiError::ApiError(i.status(), i.text().await?))
        }
    }

    pub async fn reqwest_exchange<Treq: Serialize, Tres: DeserializeOwned>(
        &self,
        url: impl FnOnce(&str) -> String,
        req: &Treq,
    ) -> Result<Tres, DawnstoreApiError> {
        let resp = reqwest::Client::new()
            .post(url(self.get_base_url()))
            .json(req)
            .send()
            .await?
            .json::<Tres>()
            .await?;
        Ok(resp)
    }
}
