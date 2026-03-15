#![allow(dead_code)]
use sqlx::{PgConnection, QueryBuilder};

use super::data_models::{ApiObjectInfo, ForeignKeyConstraint, Object, ObjectInfo, ObjectSchema, Relation};
use dawnstore_lib::*;
use dawnstore_core::abstractions::{BackendGetObjectsFilter, ForeignKeyBehaviour, ForeignKeyType};

use sqlx::{PgPool, Result};
use uuid::Uuid;

// Fetches constraints for a given api_version/kind
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

pub async fn get_all_foreign_key_constraints(pool: &PgPool) -> Result<Vec<ForeignKeyConstraint>> {
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
        "#
    )
    .fetch_all(pool)
    .await
}

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

pub async fn insert_object_schema(pool: &mut PgConnection, item: &ObjectSchema) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO object_schemas (id, api_version, kind, aliases, json_schema) VALUES ($1, $2, $3, $4, $5)",
        item.id, item.api_version, item.kind, &item.aliases, item.json_schema
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_object_schema(pool: &mut PgConnection, api_version: &str, kind: &str) -> Result<Option<ObjectSchema>, sqlx::Error> {
    sqlx::query_as!(ObjectSchema, "SELECT * FROM object_schemas WHERE kind = $1 and api_version = $2", kind, api_version)
        .fetch_optional(pool)
        .await
}

pub async fn get_all_object_schemas(pool: &sqlx::PgPool) -> Result<Vec<ObjectSchema>, sqlx::Error> {
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

    query_builder.push(" ON CONFLICT (id) DO UPDATE SET ");
    query_builder.push("api_version = EXCLUDED.api_version, ");
    query_builder.push("updated_at = EXCLUDED.updated_at, ");
    query_builder.push("annotations = EXCLUDED.annotations, ");
    query_builder.push("labels = EXCLUDED.labels, ");
    query_builder.push("spec = EXCLUDED.spec");

    query_builder.build().execute(pool).await?;
    Ok(())
}

pub async fn get_objects(pool: &mut PgConnection, ids: &[uuid::Uuid]) -> Result<Vec<Object>, sqlx::Error> {
    sqlx::query_as!(Object, "SELECT id, string_id, api_version, name, kind, created_at, updated_at, namespace, annotations as \"annotations: _\", labels as \"labels: _\", spec as \"spec: _\" FROM objects WHERE id = ANY($1)", ids)
        .fetch_all(pool)
        .await
}

pub async fn get_objects_by_filter(pool: &mut PgConnection, filter: &BackendGetObjectsFilter) -> Result<Vec<Object>, sqlx::Error> {
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

    match &filter.allowed {
        Some(scopes) if scopes.is_empty() => {
            // No permitted scopes → return nothing.
            query_builder.push(" and false");
        }
        Some(scopes) => {
            query_builder.push(" and (");
            for (i, scope) in scopes.iter().enumerate() {
                if i > 0 {
                    query_builder.push(" or ");
                }
                query_builder.push("(");
                if let Some(ns) = &scope.namespace {
                    query_builder.push("namespace = ");
                    query_builder.push_bind(ns);
                    query_builder.push(" and ");
                }
                if scope.kind == "*" {
                    query_builder.push("true");
                } else {
                    query_builder.push("kind = ");
                    query_builder.push_bind(&scope.kind);
                }
                if let Some(names) = &scope.names {
                    query_builder.push(" and name = ANY(");
                    query_builder.push_bind(names);
                    query_builder.push(")");
                }
                query_builder.push(")");
            }
            query_builder.push(")");
        }
        None => {} // unrestricted
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

/// Return the string IDs of all objects that have an inbound FK relation to the
/// object identified by `string_id`. Used by the delete handler to block deletes
/// when referencing objects exist.
pub async fn get_objects_referencing(
    pool: &mut PgConnection,
    string_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT o.string_id
        FROM relations r
        JOIN objects o ON r.object_id = o.id
        WHERE r.foreign_object_id = (
            SELECT id FROM objects WHERE string_id = $1
        )
        "#,
        string_id,
    )
    .fetch_all(pool)
    .await
}

pub async fn delete_object(pool: &mut PgConnection, namespace: Option<&str>, name: &str, kind: &str) -> Result<(), sqlx::Error> {
    let mut qb = QueryBuilder::<sqlx::Postgres>::new("DELETE FROM objects WHERE name = ");
    qb.push_bind(name).push(" and kind = ").push_bind(kind);
    if let Some(ns) = namespace {
        qb.push(" and namespace = ");
        qb.push_bind(ns);
    }
    qb.build().execute(pool).await?;
    Ok(())
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

pub async fn insert_multiple_relation(
    pool: &mut PgConnection,
    relations: &[Relation],
) -> Result<(), sqlx::Error> {
    if relations.is_empty() {
        return Ok(());
    }

    let mut query_builder = QueryBuilder::new(
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

    query_builder.build().execute(pool).await?;
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

/// Return the string IDs of objects in namespaces OTHER than `namespace` that
/// hold an inbound FK relation pointing at any object inside `namespace`.
pub async fn get_cross_namespace_inbound_references(
    pool: &mut PgConnection,
    namespace: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT DISTINCT o.string_id
        FROM relations r
        JOIN objects o ON r.object_id = o.id
        WHERE o.namespace != $1
          AND r.foreign_object_id IN (
              SELECT id FROM objects WHERE namespace = $1
          )
        "#,
        namespace,
    )
    .fetch_all(pool)
    .await
}

/// Delete all objects whose `namespace` equals `namespace`.
///
/// The `relations` table rows are removed automatically via `ON DELETE CASCADE`.
pub async fn delete_objects_by_namespace(
    pool: &mut PgConnection,
    namespace: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM objects WHERE namespace = $1", namespace)
        .execute(pool)
        .await?;
    Ok(())
}
