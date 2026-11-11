//! Демо и реальный счёт одного человека (demo-money): маппинг личностей по
//! `(tenant, sub, kind)` — живой прогон против PostgreSQL.
//!
//! Запуск, как у остальных живых: `TEST_DATABASE_URL=postgres://... cargo test
//! -p persistence -- --ignored --test-threads=1` на ЧИСТОЙ базе.

use domain::account::AccountKind;
use persistence::{MemoryStore, Persistence};

/// Без базы реального счёта не бывает: отказ, а не подмена демо-счётом.
#[tokio::test]
async fn without_a_database_there_is_no_real_account() {
    let store = MemoryStore::new();
    assert!(store
        .resolve_account("t", "s", AccountKind::Real)
        .await
        .is_err());
    assert_eq!(
        store
            .find_account("t", "s", AccountKind::Real)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL=postgres://... cargo test -p persistence -- --ignored"]
async fn postgres_keeps_demo_and_real_apart_and_migrates_old_rows() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL не задан — тест запущен без базы, которую он проверяет");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();

    // Таблица в СТАРОЙ форме, со строкой — как на стенде до разделения:
    // уникальность по (tenant, sub), колонки рода нет.
    sqlx::raw_sql(
        "DROP TABLE IF EXISTS identities;
         CREATE SEQUENCE IF NOT EXISTS identity_seq START 1000;
         CREATE TABLE identities (
           user_id BIGINT PRIMARY KEY DEFAULT nextval('identity_seq'),
           tenant TEXT NOT NULL, sub TEXT NOT NULL,
           created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
           UNIQUE (tenant, sub));
         INSERT INTO identities (user_id, tenant, sub) VALUES (4242, 'apex-ru', 'old-user');",
    )
    .execute(&pool)
    .await
    .unwrap();

    let store = persistence::PgStore::connect(&url)
        .await
        .expect("подключение");
    store.init().await.expect("DDL поверх старой формы");
    // Повторный init — идемпотентен.
    store.init().await.expect("DDL повторно");

    // Старая строка стала демо-счётом, номер сохранён — данные не потеряны.
    let old_demo = store
        .resolve_account("apex-ru", "old-user", AccountKind::Demo)
        .await
        .unwrap();
    assert_eq!(old_demo.0, 4242);
    assert_eq!(
        store.resolve_identity("apex-ru", "old-user").await.unwrap(),
        old_demo,
        "прежний вход по SSO попадает в тот же счёт"
    );
    assert_eq!(
        store.account_kind_of(old_demo).await.unwrap(),
        AccountKind::Demo
    );

    // Реальный счёт того же человека — другой номер; повтор — тот же.
    assert_eq!(
        store
            .find_account("apex-ru", "old-user", AccountKind::Real)
            .await
            .unwrap(),
        None
    );
    let real = store
        .resolve_account("apex-ru", "old-user", AccountKind::Real)
        .await
        .unwrap();
    assert_ne!(real, old_demo);
    assert_eq!(
        store
            .resolve_account("apex-ru", "old-user", AccountKind::Real)
            .await
            .unwrap(),
        real
    );
    assert_eq!(
        store
            .find_account("apex-ru", "old-user", AccountKind::Real)
            .await
            .unwrap(),
        Some(real)
    );
    assert_eq!(
        store.account_kind_of(real).await.unwrap(),
        AccountKind::Real
    );
    assert_eq!(
        store.identity_of(real).await.unwrap(),
        Some(("apex-ru".to_string(), "old-user".to_string()))
    );
}
