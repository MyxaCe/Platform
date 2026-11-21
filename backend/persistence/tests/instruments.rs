use persistence::{InstrumentRow, MemoryStore, NewInstrument, Persistence};

fn row(id: u32, symbol: &str, pd: u8, qd: u8) -> InstrumentRow {
    InstrumentRow {
        id,
        symbol: symbol.to_string(),
        source: "v1".into(),
        provider_symbol: format!("{symbol}T"),
        price_decimals: pd,
        qty_decimals: qd,
    }
}

fn new(symbol: &str, source: &str, pd: u8, qd: u8) -> NewInstrument {
    NewInstrument {
        symbol: symbol.to_string(),
        source: source.to_string(),
        provider_symbol: symbol.to_string(),
        price_decimals: pd,
        qty_decimals: qd,
    }
}

async fn registry_contract(store: &dyn Persistence, prefix: &str) {
    let btc = row(1, &format!("{prefix}BTCUSD"), 2, 5);
    let eth = row(2, &format!("{prefix}ETHUSD"), 2, 4);
    store
        .pin_instruments(&[btc.clone(), eth.clone()])
        .await
        .unwrap();
    store
        .pin_instruments(&[row(1, &format!("{prefix}BTCUSD"), 8, 8)])
        .await
        .unwrap();
    let rows = store.load_instruments().await.unwrap();
    assert!(rows.contains(&btc) && rows.contains(&eth));

    let eur = store
        .register_instrument(&new(&format!("{prefix}EURUSD"), "v2", 5, 0))
        .await
        .unwrap();
    assert!(eur.id > 30);
    assert_eq!((eur.price_decimals, eur.qty_decimals), (5, 0));

    let again = store
        .register_instrument(&new(&format!("{prefix}EURUSD"), "v2", 3, 2))
        .await
        .unwrap();
    assert_eq!(again, eur);

    let xau = store
        .register_instrument(&new(&format!("{prefix}XAUUSD"), "v2", 2, 0))
        .await
        .unwrap();
    assert!(xau.id > eur.id);
}

#[tokio::test]
async fn memory_registry_keeps_ids_and_precision_of_first_appearance() {
    let store = MemoryStore::new();
    registry_contract(&store, "").await;
}

#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL=postgres://... cargo test -p persistence -- --ignored"]
async fn postgres_registry_keeps_ids_and_precision_and_refuses_edits() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL не задан — тест запущен без базы, которую он проверяет");
    let store = persistence::PgStore::connect(&url).await.unwrap();
    store.init().await.unwrap();
    store.init().await.unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let existing: Vec<(i32, String)> =
        sqlx::query_as("SELECT id, symbol FROM instruments WHERE id IN (1, 2)")
            .fetch_all(&pool)
            .await
            .unwrap();
    let prefix = if existing.is_empty() {
        String::new()
    } else {
        let tag: i64 = sqlx::query_scalar("SELECT nextval('instrument_id_seq')")
            .fetch_one(&pool)
            .await
            .unwrap();
        format!("T{tag}")
    };
    if prefix.is_empty() {
        registry_contract(&store, "").await;
    } else {
        let eur = store
            .register_instrument(&new(&format!("{prefix}EURUSD"), "v2", 5, 0))
            .await
            .unwrap();
        let again = store
            .register_instrument(&new(&format!("{prefix}EURUSD"), "v2", 3, 2))
            .await
            .unwrap();
        assert_eq!(again, eur);
        assert!(eur.id > 30);
    }

    let target = store.load_instruments().await.unwrap().pop().unwrap();
    let upd = sqlx::query("UPDATE instruments SET price_decimals = 9 WHERE id = $1")
        .bind(target.id as i32)
        .execute(&pool)
        .await;
    assert!(upd.is_err(), "точность хранения неизменяема механизмом БД");
    let del = sqlx::query("DELETE FROM instruments WHERE id = $1")
        .bind(target.id as i32)
        .execute(&pool)
        .await;
    assert!(
        del.is_err(),
        "строка реестра не удаляется: id не переиспользуется"
    );

    let fresh = persistence::PgStore::connect(&url).await.unwrap();
    assert!(fresh.load_instruments().await.unwrap().contains(&target));
}
