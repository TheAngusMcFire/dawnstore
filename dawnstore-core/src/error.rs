use jsonschema::ValidationError;
use thiserror::Error;

use crate::models::ForeignKeyType;

#[derive(Error, Debug)]
pub enum DawnStoreError {
    #[error("Unexpected input Root object allowed are object and array")]
    InvalidRootInputObject,
    #[error("Something unexpected happened: {0}")]
    InternalServerError(String),
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
    #[error("Foreign key {0} not found")]
    ForeignKeyNotFound(String),
    #[error("No Schema for object version: {api_version} kind: {kind} found")]
    NoSchemaForObjectFound { api_version: String, kind: String },
    #[error("Database Error: {0}")]
    DatabaseError(#[from] sqlx::Error),
    #[error("Error during jsonshema creation: {0}")]
    JsonSchemaValidatorCreationError(#[from] ValidationError<'static>),
    #[error("Error during jsonshema creation of {api_version}/{kind}/{name}: {validation_error}")]
    ObjectValidationError {
        api_version: String,
        kind: String,
        name: String,
        validation_error: ValidationError<'static>,
    },
    #[error(
        "Error missing foreign key field {api_version}/{kind}/{name}: {foreign_key_path} type: {foreign_key_type:?}"
    )]
    ObjectValidationMissingForeignKeyEntry {
        api_version: String,
        kind: String,
        name: String,
        foreign_key_path: String,
        foreign_key_type: ForeignKeyType,
    },

    #[error(
        "Error wrong foreign key field {api_version}/{kind}/{name}: {foreign_key_path} type: {foreign_key_type:?} value: {value}"
    )]
    ObjectValidationWrongForeignKeyEntryFormat {
        api_version: String,
        kind: String,
        name: String,
        foreign_key_path: String,
        foreign_key_type: ForeignKeyType,
        value: String,
    },
    #[error(
        "Error wrong foreign key kind {api_version}/{kind}/{name}: {foreign_key_path} type: {foreign_key_type:?} value: {value}"
    )]
    ObjectValidationWrongForeignKeyEntryKind {
        api_version: String,
        kind: String,
        name: String,
        foreign_key_path: String,
        foreign_key_type: ForeignKeyType,
        value: String,
    },
    #[error("Error foreign key {api_version}/{kind}/{name}: value: {value} not found")]
    ObjectValidationForeignKeyNotFound {
        api_version: String,
        kind: String,
        name: String,
        value: String,
    },
}
