use sqlx::{PgConnection, Pool, Postgres, migrate::MigrateError};

use crate::{error::DawnStoreError, models::ListObject};

mod data_models;

pub async fn sqlx_migrate(pool: &Pool<Postgres>) -> Result<(), MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

pub async fn apply_raw(
    con: &mut PgConnection,
    data: serde_json::Value,
) -> Result<Vec<ListObject<serde_json::Value>>, DawnStoreError> {
    todo!()
}
