use chrono::{Duration, Utc};
use rand::{TryRngCore, rngs::OsRng};
use sha2::{Digest, Sha256};

use crate::{error::AppError, store::session::{SessionRecord, SessionStore}};

pub async fn save_session(store: &dyn SessionStore, user_id: &str, session_age_days: i64) -> Result<(String, SessionRecord), AppError> {
    let mut bytes = [0u8; 32];
    OsRng.try_fill_bytes(&mut bytes)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("failed to generate session id")))?;

    let session_id = hex::encode(bytes);

    let session_id_hash = hex::encode(Sha256::digest(session_id.as_bytes()));
    let created_at = Utc::now();
    let expires_at = created_at + Duration::days(session_age_days);

    let session = store.insert_session(SessionRecord {
        id: session_id_hash,
        user_id: user_id.to_string(),
        created_at,
        expires_at,
    }).await?;

    Ok((session_id, session))
}

pub async fn get_session(store: &dyn SessionStore, session_id: &str) -> Result<Option<SessionRecord>, AppError> {

    let session_id_hash = hex::encode(Sha256::digest(session_id.as_bytes()));

    store.get_session_by_id(&session_id_hash).await

}


#[cfg(test)]
mod tests {
    use crate::store::session::MockSessionStore;

use super::*;

    // [테스트 시나리오] 정상적으로 세션이 저장된다.
    #[tokio::test]
    async fn save_session_succeeds(){
        let mut store = MockSessionStore::new();
        store
            .expect_insert_session()
            .withf(|session_record| session_record.user_id == "user_id")
            .times(1)
            .returning(|session_record| Ok(session_record));

        let result = save_session(&store, "user_id", 30).await;
        assert!(result.is_ok());
        let (session_id, session) = result.unwrap();
        assert!(!session_id.is_empty());
        let session_id_hash = hex::encode(Sha256::digest(session_id.as_bytes()));
        assert_eq!(session_id_hash, session.id);
    }
}