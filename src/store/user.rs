use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::error::AppError;

use super::SqliteStore;

#[derive(Debug, Clone, Serialize)]
pub struct UserRecord {
    pub id: String,
    pub login_id: String,
    pub password_hash: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait UserStore: Send + Sync {
    async fn insert_user(&self, user: UserRecord) -> Result<UserRecord, AppError>;
    async fn get_all_users(&self) -> Result<Vec<UserRecord>, AppError>;
    async fn get_user_by_login_id(&self, login_id: &str) -> Result<Option<UserRecord>, AppError>;
    async fn get_user_by_id(&self, id: &str) -> Result<Option<UserRecord>, AppError>;
    async fn update_last_login_at(&self, id: &str, last_login_at: DateTime<Utc>) -> Result<(), AppError>;
}

#[async_trait]
impl UserStore for SqliteStore {
    async fn insert_user(&self, user: UserRecord) -> Result<UserRecord, AppError> {

        sqlx::query_as!(
            UserRecord,
            r#"insert into users (id, login_id, password_hash, name, created_at) values (?, ?, ?, ?, ?)
               returning id, login_id, password_hash, name, created_at as "created_at: DateTime<Utc>", last_login_at as "last_login_at: DateTime<Utc>""#,
            user.id,
            user.login_id,
            user.password_hash,
            user.name,
            user.created_at
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|err| match &err {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                AppError::Conflict(format!("login id already exists"))
            }
            _ => AppError::Database(err),
        })
    }

    async fn get_all_users(&self) -> Result<Vec<UserRecord>, AppError> {
        // [SQLite] created_at이 ISO 8601 문자열이라 문자열 정렬이 곧 시간 정렬이다.
        // 그래서 order by가 형식을 신경 쓰지 않고 그대로 성립한다.
        Ok(sqlx::query_as!(
            UserRecord,
            r#"select id, login_id, password_hash, name, created_at as "created_at: DateTime<Utc>", last_login_at as "last_login_at: DateTime<Utc>"
               from users order by created_at desc"#
        )
        .fetch_all(&self.pool)
        .await?)
    }

    async fn get_user_by_login_id(&self, login_id: &str) -> Result<Option<UserRecord>, AppError> {
        Ok(sqlx::query_as!(
            UserRecord,
            r#"select id, login_id, password_hash, name, created_at as "created_at: DateTime<Utc>", last_login_at as "last_login_at: DateTime<Utc>"
               from users where login_id = ?"#,
            login_id
        ).fetch_optional(&self.pool)
        .await?)
    }

    async fn get_user_by_id(&self, id: &str) -> Result<Option<UserRecord>, AppError> {
        Ok(sqlx::query_as!(
            UserRecord,
            r#"select id, login_id, password_hash, name, created_at as "created_at: DateTime<Utc>", last_login_at as "last_login_at: DateTime<Utc>"
               from users where id = ?"#,
            id
        ).fetch_optional(&self.pool)
        .await?)
    }

    async fn update_last_login_at(&self, id: &str, last_login_at: DateTime<Utc>) -> Result<(), AppError> {
        sqlx::query!(
            r#"update users set last_login_at = ? where id = ?"#,
            last_login_at,
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    
}
