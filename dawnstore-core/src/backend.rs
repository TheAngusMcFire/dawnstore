use std::future::Future;

use dawnstore_lib::*;

use crate::error::DawnStoreError;

pub trait DawnstoreBackend: Send + Sync {
    fn apply_raw(
        &self,
        data: serde_json::Value,
    ) -> impl Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send;

    fn get(
        &self,
        filter: &GetObjectsFilter,
    ) -> impl Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send;

    fn delete(
        &self,
        delete: &DeleteObject,
    ) -> impl Future<Output = Result<(), DawnStoreError>> + Send;

    fn get_resource_definition(
        &self,
        filter: &GetResourceDefinitionFilter,
    ) -> impl Future<Output = Result<Vec<ResourceDefinition>, DawnStoreError>> + Send;

    fn get_object_infos(
        &self,
        filter: &GetObjectInfosFilter,
    ) -> impl Future<Output = Result<ObjectInfos, DawnStoreError>> + Send;
}
