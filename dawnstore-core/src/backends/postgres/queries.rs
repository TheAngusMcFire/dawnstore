use crate::backends::postgres::data_models::{ForeignKeyConstraint, Object, ObjectSchema};

// foreign key constraint
pub async fn insert_foreign_key_constraint(pool: &sqlx::PgPool, item: &ForeignKeyConstraint) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO foreign_key_constraints (id, api_version, kind, key_path) VALUES ($1, $2, $3, $4)",
        item.id, item.api_version, item.kind, item.key_path
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_multiple_foreign_key_constraints(pool: &sqlx::PgPool, items: &[ForeignKeyConstraint]) -> Result<(), sqlx::Error> {
    let mut query_builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "INSERT INTO foreign_key_constraints (id, api_version, kind, key_path) "
    );
    query_builder.push_values(items, |mut b, item| {
        b.push_bind(item.id)
            .push_bind(&item.api_version)
            .push_bind(&item.kind)
            .push_bind(&item.key_path);
    });
    query_builder.build().execute(pool).await?;
    Ok(())
}

pub async fn get_foreign_key_constraint(pool: &sqlx::PgPool, id: uuid::Uuid) -> Result<Option<ForeignKeyConstraint>, sqlx::Error> {
    sqlx::query_as!(ForeignKeyConstraint, "SELECT * FROM foreign_key_constraints WHERE id = $1", id)
        .fetch_optional(pool)
        .await
}

pub async fn update_foreign_key_constraint(pool: &sqlx::PgPool, item: &ForeignKeyConstraint) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE foreign_key_constraints SET api_version = $2, kind = $3, key_path = $4 WHERE id = $1",
        item.id, item.api_version, item.kind, item.key_path
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_foreign_key_constraint(pool: &sqlx::PgPool, id: uuid::Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM foreign_key_constraints WHERE id = $1", id)
        .execute(pool)
        .await?;
    Ok(())
}

// object schema
pub async fn insert_object_schema(pool: &sqlx::PgPool, item: &ObjectSchema) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO object_schemas (id, api_version, kind, json_schema) VALUES ($1, $2, $3, $4)",
        item.id, item.api_version, item.kind, item.json_schema
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_multiple_object_schemas(pool: &sqlx::PgPool, items: &[ObjectSchema]) -> Result<(), sqlx::Error> {
    let mut query_builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "INSERT INTO object_schemas (id, api_version, kind, json_schema) "
    );
    query_builder.push_values(items, |mut b, item| {
        b.push_bind(item.id)
            .push_bind(&item.api_version)
            .push_bind(&item.kind)
            .push_bind(&item.json_schema);
    });
    query_builder.build().execute(pool).await?;
    Ok(())
}

pub async fn get_object_schema(pool: &sqlx::PgPool, api_version: &str, kind: &str ) -> Result<Option<ObjectSchema>, sqlx::Error> {
    sqlx::query_as!(ObjectSchema, "SELECT * FROM object_schemas WHERE kind = $1 and api_version = $2" , kind, api_version)
        .fetch_optional(pool)
        .await
}

pub async fn get_all_object_schemas(
    pool: &sqlx::PgPool,
) -> Result<Vec<ObjectSchema>, sqlx::Error> {
    sqlx::query_as!(
        ObjectSchema,
        r#"
        SELECT 
            id, 
            api_version, 
            kind, 
            json_schema 
        FROM object_schemas
        "#
    )
    .fetch_all(pool)
    .await
}

pub async fn update_object_schema(pool: &sqlx::PgPool, item: &ObjectSchema) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE object_schemas SET api_version = $2, kind = $3, json_schema = $4 WHERE id = $1",
        item.id, item.api_version, item.kind, item.json_schema
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_object_schema(pool: &sqlx::PgPool, id: uuid::Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM object_schemas WHERE id = $1", id)
        .execute(pool)
        .await?;
    Ok(())
}


// objects
pub async fn insert_object(pool: &sqlx::PgPool, item: &Object) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO objects (id, api_version, name, kind, created_at, updated_at, namespace, annotations, labels, owners, spec) 
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        item.id, item.api_version, item.name, item.kind, item.created_at, item.updated_at, item.namespace, 
        serde_json::to_value(&item.annotations).unwrap(), 
        serde_json::to_value(&item.labels).unwrap(), 
        &item.owners, item.spec.0
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_multiple_objects(pool: &sqlx::PgPool, items: &[Object]) -> Result<(), sqlx::Error> {
    let mut query_builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "INSERT INTO objects (id, api_version, name, kind, created_at, updated_at, namespace, annotations, labels, owners, spec) "
    );
    query_builder.push_values(items, |mut b, item| {
        b.push_bind(item.id)
            .push_bind(&item.api_version)
            .push_bind(&item.name)
            .push_bind(&item.kind)
            .push_bind(item.created_at)
            .push_bind(item.updated_at)
            .push_bind(&item.namespace)
            .push_bind(serde_json::to_value(&item.annotations).unwrap())
            .push_bind(serde_json::to_value(&item.labels).unwrap())
            .push_bind(&item.owners)
            .push_bind(&item.spec.0);
    });
    query_builder.build().execute(pool).await?;
    Ok(())
}

pub async fn get_object(pool: &sqlx::PgPool, id: uuid::Uuid) -> Result<Option<Object>, sqlx::Error> {
    sqlx::query_as!(Object, "SELECT id, api_version, name, kind, created_at, updated_at, namespace, annotations as \"annotations: _\", labels as \"labels: _\", owners, spec as \"spec: _\" FROM objects WHERE id = $1", id)
        .fetch_optional(pool)
        .await
}

pub async fn update_object(pool: &sqlx::PgPool, item: &Object) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE objects SET api_version = $2, name = $3, kind = $4, updated_at = $5, namespace = $6, annotations = $7, labels = $8, owners = $9, spec = $10 WHERE id = $1",
        item.id, item.api_version, item.name, item.kind, item.updated_at, item.namespace,
        serde_json::to_value(&item.annotations).unwrap(),
        serde_json::to_value(&item.labels).unwrap(),
        &item.owners, item.spec.0
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_object(pool: &sqlx::PgPool, id: uuid::Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM objects WHERE id = $1", id)
        .execute(pool)
        .await?;
    Ok(())
}


