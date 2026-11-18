//! Настройки площадки (минимальное расстояние стопов): отдельная таблица, а не
//! журнал счёта — условие отката на прежний образ.
//!
//! Живой прогон, как у остальных: `TEST_DATABASE_URL=postgres://... cargo test
//! -p persistence -- --ignored --test-threads=1`.

use persistence::{MemoryStore, Persistence};

#[tokio::test]
async fn memory_store_keeps_the_last_value_per_site() {
    let store = MemoryStore::new();
    assert!(store.load_site_min_stops().await.unwrap().is_empty());
    store.save_site_min_stop("apex-ru", 50).await.unwrap();
    store.save_site_min_stop("apex-ru", 75).await.unwrap();
    store.save_site_min_stop("b-site", 0).await.unwrap();
    assert_eq!(
        store.load_site_min_stops().await.unwrap(),
        vec![("apex-ru".to_string(), 75), ("b-site".to_string(), 0)]
    );
}

#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL=postgres://... cargo test -p persistence -- --ignored"]
async fn postgres_upserts_site_stops_and_survives_a_reconnect() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL не задан — тест запущен без базы, которую он проверяет");
    let store = persistence::PgStore::connect(&url)
        .await
        .expect("подключение");
    store.init().await.expect("DDL");
    store.init().await.expect("DDL повторно — идемпотентен");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("DELETE FROM site_settings WHERE tenant LIKE 'stops-test-%'")
        .execute(&pool)
        .await
        .unwrap();

    store.save_site_min_stop("stops-test-a", 50).await.unwrap();
    store.save_site_min_stop("stops-test-a", 120).await.unwrap();
    store.save_site_min_stop("stops-test-b", 0).await.unwrap();

    // Новое подключение — то, что увидит процесс после рестарта.
    let again = persistence::PgStore::connect(&url).await.unwrap();
    let got: Vec<_> = again
        .load_site_min_stops()
        .await
        .unwrap()
        .into_iter()
        .filter(|(t, _)| t.starts_with("stops-test-"))
        .collect();
    assert_eq!(
        got,
        vec![
            ("stops-test-a".to_string(), 120),
            ("stops-test-b".to_string(), 0)
        ]
    );

    // Граница значений держится и самой таблицей, а не только ручкой.
    assert!(again
        .save_site_min_stop("stops-test-a", 10_001)
        .await
        .is_err());
    assert!(again.save_site_min_stop("stops-test-a", -1).await.is_err());
}
