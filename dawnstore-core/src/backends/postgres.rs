use sqlx::{Pool, Postgres, migrate::MigrateError};

mod data_models;

pub async fn sqlx_migrate(pool: &Pool<Postgres>) -> Result<(), MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
