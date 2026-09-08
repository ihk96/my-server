use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::{error::AppError, store::SqliteStore};


pub struct SessionRecord {
    pub id: String,
    pub user_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl SessionRecord {
    pub fn is_expired(&self) -> bool {
        self.expires_at.lt(&Utc::now())
    }
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn insert_session(&self, session: SessionRecord) -> Result<SessionRecord, AppError>;
    async fn get_session_by_id(&self, id: &str) -> Result<Option<SessionRecord>, AppError>;
    async fn get_sessions_by_user_id(&self, user_id: &str) -> Result<Vec<SessionRecord>, AppError>;
    async fn delete_session_by_id(&self, id: &str) -> Result<(), AppError>;
    async fn delete_sessions_by_user_id(&self, user_id: &str) -> Result<(), AppError>;
    async fn delete_expired_sessions(&self) -> Result<(), AppError>;
}

#[async_trait]
impl SessionStore for SqliteStore {
    async fn insert_session(&self, session: SessionRecord) -> Result<SessionRecord, AppError> {
        sqlx::query_as!(
            SessionRecord,
            r#"insert into sessions (id, user_id, created_at, expires_at) values (?, ?, ?, ?)
               returning id, user_id, created_at as "created_at: DateTime<Utc>", expires_at as "expires_at: DateTime<Utc>""#,
            session.id,
            session.user_id,
            session.created_at,
            session.expires_at
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)
    }

    async fn get_session_by_id(&self, id: &str) -> Result<Option<SessionRecord>, AppError> {
        sqlx::query_as!(
            SessionRecord,
            r#"select id, user_id, created_at as "created_at: DateTime<Utc>", expires_at as "expires_at: DateTime<Utc>"
               from sessions where id = ?"#,
            id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::Database)
    }
    async fn get_sessions_by_user_id(&self, user_id: &str) -> Result<Vec<SessionRecord>, AppError> {
        sqlx::query_as!(
            SessionRecord,
            r#"select id, user_id, created_at as "created_at: DateTime<Utc>", expires_at as "expires_at: DateTime<Utc>"
               from sessions where user_id = ?"#,
            user_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)
    }
    async fn delete_session_by_id(&self, id: &str) -> Result<(), AppError> {
        sqlx::query!(
            r#"delete from sessions where id = ?"#,
            id
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::Database)?;
        Ok(())
    }
    async fn delete_sessions_by_user_id(&self, user_id: &str) -> Result<(), AppError> {
        sqlx::query!(
            r#"delete from sessions where user_id = ?"#,
            user_id
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::Database)?;
        Ok(())
    }
    async fn delete_expired_sessions(&self) -> Result<(), AppError> {
        let now = Utc::now();
        sqlx::query!(
            r#"delete from sessions where expires_at < ?"#,
            now
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::Database)?;
        Ok(())
    }
}