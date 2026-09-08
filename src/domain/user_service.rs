
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::Utc;
use uuid::Uuid;

use crate::{error::AppError, store::{UserRecord, UserStore}};

pub struct CreateUserInput {
    pub login_id: String,
    pub password: String,
    pub name: String,
}

pub async fn create_user(store: &dyn UserStore, input: CreateUserInput) -> Result<UserRecord, AppError> {
    let login_id = input.login_id.trim().to_string();
    let password = input.password;
    let name = input.name.trim().to_string();

    if login_id.is_empty() {
        return Err(AppError::BadRequest("login id must not be empty".into()));
    }
    if password.is_empty() {
        return Err(AppError::BadRequest("password must not be empty".into()));
    }
    if name.is_empty() {
        return Err(AppError::BadRequest("name must not be empty".into()));
    }

    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now();

    let password_hash = tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
    })
    .await
    .map_err(|err| AppError::Internal(anyhow::anyhow!("password hashing task panicked: {err}")))?
    .map_err(|err| AppError::Internal(anyhow::anyhow!("failed to hash password: {err}")))?;

    store.insert_user(UserRecord {
        id: id,
        login_id: login_id,
        password_hash: password_hash,
        name: name,
        created_at: created_at,
        last_login_at: None,
    }).await
}

pub async fn verify_login(store: &dyn UserStore, login_id: &str, password: &str) -> Result<UserRecord, AppError> {
    let login_id = login_id.trim().to_string();
    let password = password.to_string();

    if login_id.is_empty() || password.is_empty() {
        return Err(AppError::BadRequest("login id and password must not be empty".into()));
    }

    let user = match store.get_user_by_login_id(&login_id).await? {
        Some(user) => user,
        None => return Err(AppError::Unauthorized),
    };
    
    let password_hash = user.password_hash.clone();
    let verify_result : Result<bool, AppError> = tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&password_hash)
            .map_err(|err| AppError::Internal(anyhow::anyhow!("Failed to parse password hash: {err}")))?;
        Ok(Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
    })
    .await
    .map_err(|err| AppError::Internal(anyhow::anyhow!("Failed to verify password: {err}")))?;

    if verify_result? {
        Ok(user)
    } else {
        Err(AppError::Unauthorized)
    }
}

pub async fn update_last_login(store: &dyn UserStore, user_id: &str) -> Result<(), AppError> {
    let now = Utc::now();
    store.update_last_login_at(user_id, now).await
}

pub async fn do_login(store: &dyn UserStore, login_id: &str, password: &str) -> Result<String, AppError> {
    let user = verify_login(store, login_id, password).await?;
    update_last_login(store, &user.id).await?;
    Ok(user.id)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::user::MockUserStore;

    // [테스트 시나리오] 각 필드별 공백 검증 — 저장소에 접근하지 않고 바로 BadRequest를 반환하는지 확인한다.
    #[tokio::test]
    async fn create_user_rejects_blank_required_fields() {
        let store = MockUserStore::new();

        let result1 = create_user(&store, CreateUserInput {
            login_id: "test".into(),
            password: "password".into(),
            name: "   ".into(),
        }).await;

        let result2 = create_user(&store, CreateUserInput {
            login_id: "test".into(),
            password: "".into(),
            name: "   ".into(),
        }).await;

        let result3 = create_user(&store, CreateUserInput {
            login_id: "".into(),
            password: "password".into(),
            name: "   ".into(),
        }).await;

        assert!(matches!(result1, Err(AppError::BadRequest(_))));
        assert!(matches!(result2, Err(AppError::BadRequest(_))));
        assert!(matches!(result3, Err(AppError::BadRequest(_))));

    }

    // [테스트 시나리오] 입력값의 공백이 제거되어 저장소에 전달되는지 확인한다. `withf`를 이용해 mock expectation을 설정하고, 실제로 호출될 때 전달되는 값이 기대한 값과 일치하는지 검증한다.
    // 값을 `withf`로 들여다본다.
    #[tokio::test]
    async fn create_user_succeeds_with_trimmed_fields() {
        let mut store = MockUserStore::new();
        store
            .expect_insert_user()
            .withf(|user| user.name == "hello" && user.login_id == "test" && user.password_hash.len() > 0)
            .times(1)
            .returning(|user| {
                Ok(UserRecord {
                    id: user.id,
                    login_id: user.login_id,
                    password_hash: user.password_hash,
                    name: user.name,
                    created_at: user.created_at,
                    last_login_at: user.last_login_at,
                })
            });

        let created = create_user(&store, CreateUserInput {
            login_id: "test".into(),
            password: "password".into(),
            name: "  hello  ".into(),
        }).await.unwrap();

        assert_eq!(created.name, "hello");
        assert_eq!(created.login_id, "test");
        assert!(!created.password_hash.is_empty());
    }


    // [테스트 시나리오] 비어있는 로그인 ID 또는 비밀번호로 로그인 시도 시 BadRequest를 반환하는지 확인한다.
    #[tokio::test]
    async fn verify_login_rejects_blank_fields() {
        let store = MockUserStore::new();

        let result1 = verify_login(&store, "", "password").await;
        let result2 = verify_login(&store, "login", "").await;
        let result3 = verify_login(&store, "   ", "   ").await;

        assert!(matches!(result1, Err(AppError::BadRequest(_))));
        assert!(matches!(result2, Err(AppError::BadRequest(_))));
        assert!(matches!(result3, Err(AppError::BadRequest(_))));
    }

    // [테스트 시나리오] 없는 로그인 ID로 로그인 시도 시 Unauthorized 반환하는지 확인한다.
    #[tokio::test]
    async fn verify_login_fails_with_not_found_id() {
        let mut store = MockUserStore::new();
        store
            .expect_get_user_by_login_id()
            .withf(|login_id| login_id == "notfoundid")
            .times(1)
            .returning(|_| {
                Ok(None)
            });

        let result = verify_login(&store, "notfoundid", "wrongpassword").await;
        assert!(matches!(result, Err(AppError::Unauthorized)));
    }

    // [테스트 시나리오] 올바른 로그인 ID이지만 잘못된 비밀번호로 로그인 시도 시 Unauthorized 반환하는지 확인한다.
    #[tokio::test]
    async fn verify_login_fails_with_wrong_password() {
        let mut store = MockUserStore::new();
        let password_hash = Argon2::default().hash_password("correctpassword".as_bytes()).unwrap().to_string();

        store
            .expect_get_user_by_login_id()
            .withf(|login_id| login_id == "myid")
            .times(1)
            .returning(move |login_id| Ok(Some(UserRecord {
                id: "user-id".to_string(),
                login_id: login_id.to_string(),
                password_hash: password_hash.clone(),
                name: "My Name".to_string(),
                created_at: Utc::now(),
                last_login_at: None,
            })));

        let result = verify_login(&store, "myid", "wrongpassword").await;
        assert!(matches!(result, Err(AppError::Unauthorized)));

    }

    // [테스트 시나리오] 올바른 로그인 ID와 비밀번호로 로그인 시도 시 성공하는지 확인한다.
    #[tokio::test]
    async fn verify_login_succeeds_with_correct_credentials() {
        let mut store = MockUserStore::new();
        let password_hash = Argon2::default().hash_password("mypassword".as_bytes()).unwrap().to_string();

        store
            .expect_get_user_by_login_id()
            .withf(|login_id| login_id == "myid")
            .times(1)
            .returning(move |login_id| Ok(Some(UserRecord {
                id: "user-id".to_string(),
                login_id: login_id.to_string(),
                password_hash: password_hash.clone(),
                name: "My Name".to_string(),
                created_at: Utc::now(),
                last_login_at: None,
            })));

        let result = verify_login(&store, "myid", "mypassword").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().login_id, "myid");

    }

    // [테스트 시나리오] 마지막 로그인 시간 업데이트가 정상적으로 호출되는지 확인한다.
    #[tokio::test]
    async fn update_last_login_calls_store() {
        let mut store = MockUserStore::new();
        store
            .expect_update_last_login_at()
            .withf(|id, _| id == "user-id")
            .times(1)
            .returning(|_, _| Ok(()));

        let result = update_last_login(&store, "user-id").await;
        assert!(result.is_ok());
    }

    // [테스트 시나리오] do_login이 정상적으로 로그인하고 마지막 로그인 시간을 업데이트하는지 확인한다.
    #[tokio::test]
    async fn do_login_succeeds_and_updates_last_login() {
        let mut store = MockUserStore::new();
        let password_hash = Argon2::default().hash_password("mypassword".as_bytes()).unwrap().to_string();

        store
            .expect_get_user_by_login_id()
            .withf(|login_id| login_id == "myid")
            .times(1)
            .returning(move |login_id| Ok(Some(UserRecord {
                id: "user-id".to_string(),
                login_id: login_id.to_string(),
                password_hash: password_hash.clone(),
                name: "My Name".to_string(),
                created_at: Utc::now(),
                last_login_at: None,
            })));

        store
            .expect_update_last_login_at()
            .withf(|id, _| id == "user-id")
            .times(1)
            .returning(|_, _| Ok(()));

        let result = do_login(&store, "myid", "mypassword").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "user-id");
    }

    // [테스트 시나리오] 로그인 실패 시 마지막 로그인 시간 업데이트가 호출되지 않는지 확인한다.
    #[tokio::test]
    async fn do_login_fails_without_updating_last_login() {
        let mut store = MockUserStore::new();
        let password_hash = Argon2::default().hash_password("mypassword".as_bytes()).unwrap().to_string();

        store
            .expect_get_user_by_login_id()
            .withf(|login_id| login_id == "myid")
            .times(1)
            .returning(move |login_id| Ok(Some(UserRecord {
                id: "user-id".to_string(),
                login_id: login_id.to_string(),
                password_hash: password_hash.clone(),
                name: "My Name".to_string(),
                created_at: Utc::now(),
                last_login_at: None,
            })));

        store
            .expect_update_last_login_at()
            .times(0); // Should not be called

        let result = do_login(&store, "myid", "wrongpassword").await;
        assert!(matches!(result, Err(AppError::Unauthorized))); 
    }

}
