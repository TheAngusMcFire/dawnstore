#![allow(dead_code)]
use sqlx::{PgConnection, QueryBuilder};

use crate::backends::postgres::data_models::{ApiObjectInfo, ForeignKeyConstraint, Object, ObjectInfo, ObjectSchema, Relation};
use dawnstore_lib::*;

// foreign key constraint
use sqlx::{PgPool, Result};
use uuid::Uuid;
use crate::models::{ForeignKeyType, ForeignKeyBehaviour};

// Fetches a single constraint by ID
 pub async fn get_foreign_key_constraints(
     pool: &mut PgConnection, api_version: &str, kind: &str) -> Result<Vec<ForeignKeyConstraint>> {
     sqlx::query_as!(
         ForeignKeyConstraint,
         r#"
         SELECT 
             id, 
             api_version, 
             kind, 
             key_path, 
             parent_key_path, 
             type as "type: ForeignKeyType", 
             behaviour as "behaviour: ForeignKeyBehaviour", 
             foreign_key_kind 
         FROM foreign_key_constraints 
         WHERE api_version = $1 and kind = $2
         "#,
         api_version, kind
     )
     .fetch_all(pool)
     .await
 }

/// Inserts a single record
pub async fn insert_foreign_key_constraints(
    pool: &PgPool, 
    row: &ForeignKeyConstraint
) -> Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO foreign_key_constraints (id, api_version, kind, key_path, parent_key_path, type, behaviour, foreign_key_kind)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
        row.id, 
        row.api_version, 
        row.kind, 
        row.key_path, 
        row.parent_key_path, 
        &row.r#type as &ForeignKeyType, 
        &row.behaviour as &ForeignKeyBehaviour, 
        row.foreign_key_kind
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Inserts multiple records within a single transaction
pub async fn insert_multiple_foreign_key_constraints(
    pool: &mut PgConnection, 
    rows: &[ForeignKeyConstraint]
) -> Result<()> {
    for row in rows {
        sqlx::query!(
            r#"
            INSERT INTO foreign_key_constraints (id, api_version, kind, key_path, parent_key_path, type, behaviour, foreign_key_kind)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            row.id, 
            row.api_version, 
            row.kind, 
            row.key_path, 
            row.parent_key_path, 
            &row.r#type as &ForeignKeyType, 
            &row.behaviour as &ForeignKeyBehaviour, 
            row.foreign_key_kind
        )
        .execute(&mut *pool)
        .await?;
    }
    Ok(())
}

/// Updates an existing record based on ID
pub async fn update_foreign_key_constraints(
    pool: &PgPool, 
    row: &ForeignKeyConstraint
) -> Result<bool> {
    let result = sqlx::query!(
        r#"
        UPDATE foreign_key_constraints 
        SET api_version = $2, kind = $3, key_path = $4, parent_key_path = $5, type = $6, behaviour = $7, foreign_key_kind = $8
        WHERE id = $1
        "#,
        row.id, 
        row.api_version, 
        row.kind, 
        row.key_path, 
        row.parent_key_path, 
        &row.r#type as &ForeignKeyType, 
        &row.behaviour as &ForeignKeyBehaviour, 
        row.foreign_key_kind
    )
    .execute(pool)
    .await?;
    
    Ok(result.rows_affected() > 0)
}

/// Deletes a record by ID
pub async fn delete_foreign_key_constraints(
    pool: &PgPool, 
    id: Uuid
) -> Result<bool> {
    let result = sqlx::query!(
        "DELETE FROM foreign_key_constraints WHERE id = $1",
        id
    )
    .execute(pool)
    .await?;
    
    Ok(result.rows_affected() > 0)
}

// object schema
pub async fn insert_object_schema(pool: &mut PgConnection, item: &ObjectSchema) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO object_schemas (id, api_version, kind, aliases, json_schema) VALUES ($1, $2, $3, $4, $5)",
        item.id, item.api_version, item.kind, &item.aliases, item.json_schema
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

pub async fn get_object_schema(pool: &mut PgConnection, api_version: &str, kind: &str ) -> Result<Option<ObjectSchema>, sqlx::Error> {
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
            aliases,
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
        "INSERT INTO objects (id, api_version, name, kind, created_at, updated_at, namespace, annotations, labels, spec) 
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        item.id, item.api_version, item.name, item.kind, item.created_at, item.updated_at, item.namespace, 
        serde_json::to_value(&item.annotations).unwrap(), 
        serde_json::to_value(&item.labels).unwrap(), item.spec.0
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_multiple_objects(pool: &mut PgConnection, items: &[Object]) -> Result<(), sqlx::Error> {
    if items.is_empty() {
        return Ok(())
    }
    let mut query_builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "INSERT INTO objects (id, string_id, api_version, name, kind, created_at, updated_at, namespace, annotations, labels, spec) "
    );
    query_builder.push_values(items, |mut b, item| {
        b.push_bind(item.id)
            .push_bind(&item.string_id)
            .push_bind(&item.api_version)
            .push_bind(&item.name)
            .push_bind(&item.kind)
            .push_bind(item.created_at)
            .push_bind(item.updated_at)
            .push_bind(&item.namespace)
            .push_bind(serde_json::to_value(&item.annotations).unwrap())
            .push_bind(serde_json::to_value(&item.labels).unwrap())
            .push_bind(&item.spec.0);
    });
    query_builder.build().execute(pool).await?;
    Ok(())
}

pub async fn insert_or_update_multiple_objects(pool: &mut PgConnection, items: &[Object]) -> Result<(), sqlx::Error> {
    if items.is_empty() {
        return Ok(())
    }
    let mut query_builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "INSERT INTO objects (id, string_id, api_version, name, kind, created_at, updated_at, namespace, annotations, labels, spec) "
    );

    query_builder.push_values(items, |mut b, item| {
        b.push_bind(item.id)
            .push_bind(&item.string_id)
            .push_bind(&item.api_version)
            .push_bind(&item.name)
            .push_bind(&item.kind)
            .push_bind(item.created_at)
            .push_bind(item.updated_at)
            .push_bind(&item.namespace)
            .push_bind(serde_json::to_value(&item.annotations).unwrap())
            .push_bind(serde_json::to_value(&item.labels).unwrap())
            .push_bind(&item.spec.0);
    });

    query_builder.push(
        " ON CONFLICT (id) DO UPDATE SET "
    );

    query_builder.push("api_version = EXCLUDED.api_version, ");
    query_builder.push("updated_at = EXCLUDED.updated_at, ");
    query_builder.push("annotations = EXCLUDED.annotations, ");
    query_builder.push("labels = EXCLUDED.labels, ");
    query_builder.push("spec = EXCLUDED.spec");

    let query = query_builder.build();
    query.execute(pool).await?;

    Ok(())
}

pub async fn update_multiple_objects(pool: &mut PgConnection, items: &[Object]) -> Result<(), sqlx::Error> {
    if items.is_empty() {
        return Ok(());
    }

    let mut query_builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "UPDATE objects AS o SET 
            string_id = v.string_id,
            api_version = v.api_version,
            name = v.name,
            kind = v.kind,
            updated_at = v.updated_at,
            namespace = v.namespace,
            annotations = v.annotations,
            labels = v.labels,
            spec = v.spec
        FROM ( "
    );

    query_builder.push_values(items, |mut b, item| {
        b.push_bind(item.id)
            .push_bind(&item.string_id)
            .push_bind(&item.api_version)
            .push_bind(&item.name)
            .push_bind(&item.kind)
            .push_bind(item.updated_at)
            .push_bind(&item.namespace)
            .push_bind(serde_json::to_value(&item.annotations).unwrap())
            .push_bind(serde_json::to_value(&item.labels).unwrap())
            .push_bind(&item.spec.0);
    });

    query_builder.push(
        ") AS v(id, string_id, api_version, name, kind, updated_at, namespace, annotations, labels, spec) 
         WHERE o.id = v.id"
    );

    query_builder.build().execute(pool).await?;

    Ok(())
}

pub async fn get_object(pool: &mut PgConnection, id: uuid::Uuid) -> Result<Option<Object>, sqlx::Error> {
    sqlx::query_as!(Object, "SELECT id, string_id, api_version, name, kind, created_at, updated_at, namespace, annotations as \"annotations: _\", labels as \"labels: _\", spec as \"spec: _\" FROM objects WHERE id = $1", id)
        .fetch_optional(pool)
        .await
}

pub async fn get_objects(pool: &mut PgConnection, ids: &[uuid::Uuid]) -> Result<Vec<Object>, sqlx::Error> {
    sqlx::query_as!(Object, "SELECT id, string_id, api_version, name, kind, created_at, updated_at, namespace, annotations as \"annotations: _\", labels as \"labels: _\", spec as \"spec: _\" FROM objects WHERE id = ANY($1)", ids)
        .fetch_all(pool)
        .await
}

pub async fn object_exists(pool: &mut PgConnection, string_id: &str) -> Result<bool, sqlx::Error> {
    sqlx::query("SELECT 1 FROM objects WHERE string_id = $1")
        .bind(string_id)
        .fetch_optional(pool)
        .await
        .map(|x| x.is_some())
}

pub async fn get_objects_by_filter(pool: &mut PgConnection, filter: &GetObjectsFilter) -> Result<Vec<Object>, sqlx::Error> {
    let mut query_builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "SELECT id, string_id, api_version, name, kind, created_at, updated_at, namespace, annotations, labels, spec FROM objects where true "
    );

    if let Some(x) = &filter.namespace {
        query_builder.push(" and namespace = ");
        query_builder.push_bind(x);
    }

    if let Some(x) = &filter.ids {
        query_builder.push(" and id = ANY(");
        query_builder.push_bind(x);
        query_builder.push(") ");
    }

    if let Some(x) = &filter.kind {
        query_builder.push(" and kind = ");
        query_builder.push_bind(x);
    }

    if let Some(x) = &filter.name {
        query_builder.push(" and name = ");
        query_builder.push_bind(x);
    }

    query_builder.push(" order by kind, name");

    if let Some(x) = &filter.page_size {
        let size = (*x).min(250);
        query_builder.push(" limit ");
        query_builder.push_bind(size as i64);
    }

    if let Some(x) = &filter.page {
        let size = filter.page_size.unwrap_or(250);
        query_builder.push(" offset ");
        query_builder.push_bind((x * size) as i64);
    }

    query_builder.build_query_as::<Object>().fetch_all(pool).await
}

pub async fn get_object_infos(pool: &mut PgConnection, string_ids: &[String]) -> Result<Vec<ObjectInfo>, sqlx::Error> {
    sqlx::query_as!(ObjectInfo, "SELECT id, string_id, created_at FROM objects WHERE string_id = ANY($1)", string_ids)
        .fetch_all(pool)
        .await
}

pub async fn get_api_object_infos_with_filter(pool: &mut PgConnection, filter: &GetObjectInfosFilter) -> Result<Vec<ApiObjectInfo>, sqlx::Error> {
    let mut query_builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "SELECT namespace, id, api_version, name, kind where true "
    );

    if let Some(x) = &filter.namespace {
        query_builder.push(" and namespace = ");
        query_builder.push_bind(x);
    }

    if let Some(x) = &filter.kind {
        query_builder.push(" and kind = ");
        query_builder.push_bind(x);
    }

    if let Some(x) = &filter.name {
        query_builder.push(" and name = ");
        query_builder.push_bind(x);
    }

    if let Some(x) = &filter.name_search_string {
        query_builder.push(" and name ilike '%");
        query_builder.push_bind(x);
        query_builder.push("%' ");
    }

    query_builder.push(" order by kind, name ");

    if let Some(x) = &filter.page_size {
        let size = (*x).min(250);
        query_builder.push(" limit ");
        query_builder.push_bind(size as i64);
    }

    if let Some(x) = &filter.page {
        let size = filter.page_size.unwrap_or(250);
        query_builder.push(" offset ");
        query_builder.push_bind((x * size) as i64);
    }

    query_builder.build_query_as::<ApiObjectInfo>().fetch_all(pool).await
}

pub async fn update_object(pool: &sqlx::PgPool, item: &Object) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE objects SET api_version = $2, name = $3, kind = $4, updated_at = $5, namespace = $6, annotations = $7, labels = $8, spec = $9 WHERE id = $1",
        item.id, item.api_version, item.name, item.kind, item.updated_at, item.namespace,
        serde_json::to_value(&item.annotations).unwrap(),
        serde_json::to_value(&item.labels).unwrap(),
        item.spec.0
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_object(pool: &mut PgConnection, namespace: Option<&str>, name: &str, kind: &str) -> Result<(), sqlx::Error> {
    let mut qb = QueryBuilder::<sqlx::Postgres>::new("DELETE FROM objects WHERE name = ");
    qb.push_bind(name).push(" and kind = ").push_bind(kind);
    if let Some(ns) = namespace {
         qb.push(" and namespace = ");
         qb.push_bind(ns);
    }
    dbg!(qb.sql());
    qb.build().execute( pool).await?;
    Ok(())
}

pub async fn get_relation(
    pool: &mut PgConnection,
    object_id: Uuid,
    foreign_object_id: Uuid,
    foreign_key_id: Uuid,
) -> Result<Option<Relation>, sqlx::Error> {
    sqlx::query_as!(
        Relation,
        r#"
        SELECT object_id, foreign_object_id, foreign_key_id 
        FROM relations 
        WHERE object_id = $1 AND foreign_object_id = $2 AND foreign_key_id = $3
        "#,
        object_id,
        foreign_object_id,
        foreign_key_id
    )
    .fetch_optional(pool)
    .await
}

pub async fn get_relations_of_objects(
    pool: &mut PgConnection,
    object_ids: &[Uuid],
) -> Result<Vec<Relation>, sqlx::Error> {
    sqlx::query_as!(
        Relation,
        r#"
        SELECT object_id, foreign_object_id, foreign_key_id 
        FROM relations 
        WHERE object_id = ANY($1)
        "#,
        object_ids,
    )
    .fetch_all(pool)
    .await
}

/// Inserts the current Relation instance into the database
pub async fn insert_relation(pool: &mut PgConnection, relation: &Relation) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO relations (object_id, foreign_object_id, foreign_key_id)
        VALUES ($1, $2, $3)
        "#,
        relation.object_id,
        relation.foreign_object_id,
        relation.foreign_key_id
    )
    .execute(pool)
    .await?;
    
    Ok(())
}

/// Deletes a relation by its composite primary key
pub async fn delete_relation(
    pool: &mut PgConnection,
    object_id: Uuid,
    foreign_object_id: Uuid,
    foreign_key_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        r#"
        DELETE FROM relations 
        WHERE object_id = $1 AND foreign_object_id = $2 AND foreign_key_id = $3
        "#,
        object_id,
        foreign_object_id,
        foreign_key_id
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

pub async fn insert_multiple_relation(
    pool: &mut PgConnection,
    relations: &[Relation],
) -> Result<(), sqlx::Error> {
    if relations.is_empty() {
        return Ok(());
    }

    let mut query_builder  = QueryBuilder::new(
        "INSERT INTO relations (object_id, foreign_object_id, foreign_key_id) "
    );

    query_builder.push_values(relations, |mut b, rel| {
        b.push_bind(rel.object_id)
         .push_bind(rel.foreign_object_id)
         .push_bind(rel.foreign_key_id);
    });

    query_builder.push(
        " ON CONFLICT (object_id, foreign_object_id, foreign_key_id) DO NOTHING "
    );

    let query = query_builder.build();
    query.execute(pool).await?;

    Ok(())
}


pub async fn delete_multiple_relations(
    pool: &mut PgConnection,
    object_ids: &[Uuid],
    foreign_object_ids: &[Uuid],
    foreign_key_ids: &[Uuid],
) -> Result<u64, sqlx::Error> {
    assert_eq!(object_ids.len(), foreign_object_ids.len());
    assert_eq!(object_ids.len(), foreign_key_ids.len());

    let result = sqlx::query!(
        r#"
        DELETE FROM relations
        WHERE (object_id, foreign_object_id, foreign_key_id) IN (
            SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::uuid[])
        )
        "#,
        object_ids,
        foreign_object_ids,
        foreign_key_ids
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}
