use thiserror::Error;

#[derive(Error, Debug)]
pub enum DawnStoreError {
    #[error("Unexpected input Root object allowed are object and array")]
    InvalidRootInputObject,
    #[error("Unexpected input object missing kind field")]
    InvalidInputObjectMissingKindField,
    #[error("Unexpected input object list missing the list field")]
    InvalidInputObjectMissingListFieldOfList,
    #[error("Error during deserialization: {0}")]
    DeserialisationError(#[from] serde_json::Error),
}
