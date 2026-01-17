use dawnstore_lib::*;
use reqwest::Client;

#[derive(thiserror::Error, Debug)]
pub enum DawnstoreApiError {
    #[error("Error from reqwest: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("Error from api: {0}")]
    ApiError(String),
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

    pub async fn get_resource_definitions(
        &self,
        filter: &GetResourceDefinitionFilter,
    ) -> Result<Vec<ResourceDefinition>, DawnstoreApiError> {
        let i = self
            .client
            .get(format!("{}/get-resource-definitions", self.base_url))
            .json(filter)
            .send()
            .await?;
        if i.status().is_success() {
            Ok(i.json::<Vec<ResourceDefinition>>().await?)
        } else {
            Err(DawnstoreApiError::ApiError(i.text().await?))
        }
    }
}
