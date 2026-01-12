use jsonschema::ValidationError;
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
    #[error("Kind field is missing in object")]
    KindMissingInObject,
    #[error("ApiVersion field is missing in object")]
    ApiVersionMissingInObject,
    #[error("No Schema for object version: {api_version} kind: {kind} found")]
    NoSchemaForObjectFound { api_version: String, kind: String },
    #[error("Database Error: {0}")]
    DatabaseError(#[from] sqlx::Error),
    #[error("Error during jsonshema creation")]
    JsonShemaValidatorCreationError(#[from] ValidationError<'static>),
}
