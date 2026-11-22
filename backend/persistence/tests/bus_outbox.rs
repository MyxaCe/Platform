use broker::journal::{EventKind, Journal, Origin};
use broker::AccountSnapshot;
use domain::account::UserId;
use persistence::{MemoryStore, OutboxRecord, Persistence};

const U: UserId = UserId(91);

fn snapshot(balance: i128) -> AccountSnapshot {
    AccountSnapshot {
        profile: Default::default(),
        balance,
        next_id: 1,
        ..Default::default()
    }
}

fn opening(j: &Journal) -> Vec<broker::journal::AccountEvent> {
    j.seal(
        1_700_000_000_000,
        &Origin::Client,
        &[EventKind::AccountOpened {
            balance: 10_000_000,
            profile: Default::default(),
        }],
    )
}

fn rec(id: &str) -> OutboxRecord {
    OutboxRecord {
        event_id: id.to_string(),
        routing_key: "terminal.trade.executed".into(),
        payload: format!("{{\"event_id\":\"{id}\",\"event\":\"terminal.trade.executed\"}}"),
    }
}

#[tokio::test]
async fn saved_envelope_is_offered_to_the_relay() {
    let store = MemoryStore::new();
    let j = Journal::new(10_000_000);
    store
        .save_account(U, &snapshot(10_000_000), &opening(&j), &[rec("e-1")])
        .await
        .expect("запись");

    assert_eq!(store.outbox_backlog().await.unwrap(), 1);
    let claimed = store.claim_outbox(10, 30_000).await.expect("выборка");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].event_id, "e-1");
    assert_eq!(claimed[0].attempts, 0, "ещё не пробовали");
}

#[tokio::test]
async fn published_row_is_not_offered_again() {
    let store = MemoryStore::new();
    let j = Journal::new(10_000_000);
    store
        .save_account(U, &snapshot(10_000_000), &opening(&j), &[rec("e-1")])
        .await
        .unwrap();
    let claimed = store.claim_outbox(10, 0).await.unwrap();
    store.mark_published(claimed[0].id).await.unwrap();

    assert_eq!(store.outbox_backlog().await.unwrap(), 0);
    assert!(
        store.claim_outbox(10, 0).await.unwrap().is_empty(),
        "отправленное повторно не берут"
    );
}

#[tokio::test]
async fn failure_is_written_into_the_row() {
    let store = MemoryStore::new();
    let j = Journal::new(10_000_000);
    store
        .save_account(U, &snapshot(10_000_000), &opening(&j), &[rec("e-1")])
        .await
        .unwrap();
    let claimed = store.claim_outbox(10, 0).await.unwrap();
    store
        .mark_failed(claimed[0].id, 1, 5_000, "шина недоступна")
        .await
        .unwrap();

    let dump = store.outbox_dump();
    assert_eq!(dump[0].1, 1, "попытка обязана быть записана");
    assert!(!dump[0].2, "неотправленное отправленным не считается");
    assert_eq!(dump[0].3.as_deref(), Some("шина недоступна"));
    assert_eq!(
        store.outbox_backlog().await.unwrap(),
        1,
        "строка остаётся в очереди — событие обязано дойти, когда шина вернётся"
    );
    assert!(
        store.claim_outbox(10, 0).await.unwrap().is_empty(),
        "до срока следующей попытки строку не берут"
    );
}

#[tokio::test]
async fn claimed_row_is_leased() {
    let store = MemoryStore::new();
    let j = Journal::new(10_000_000);
    store
        .save_account(U, &snapshot(10_000_000), &opening(&j), &[rec("e-1")])
        .await
        .unwrap();
    assert_eq!(store.claim_outbox(10, 30_000).await.unwrap().len(), 1);
    assert!(
        store.claim_outbox(10, 30_000).await.unwrap().is_empty(),
        "взятое под арендой повторно не выдают"
    );
    let dump = store.outbox_dump();
    assert_eq!(
        dump[0].1, 0,
        "аренда — не попытка: счётчик отказов не трогаем"
    );
}

#[tokio::test]
async fn the_same_fact_written_twice_is_one_row() {
    let store = MemoryStore::new();
    let j = Journal::new(10_000_000);
    let sealed = opening(&j);
    store
        .save_account(U, &snapshot(10_000_000), &sealed, &[rec("e-1")])
        .await
        .unwrap();
    let mut j2 = Journal::new(10_000_000);
    for e in sealed {
        j2.commit(e).unwrap();
    }
    let next = j2.seal(
        1_700_000_050_000,
        &Origin::Client,
        &[EventKind::Deposited { amount: 1 }],
    );
    store
        .save_account(U, &snapshot(10_000_001), &next, &[rec("e-1")])
        .await
        .unwrap();

    assert_eq!(store.outbox_dump().len(), 1, "тот же факт — одна строка");
}

#[tokio::test]
async fn rejected_write_leaves_no_envelope() {
    let store = MemoryStore::new();
    let j = Journal::new(10_000_000);
    let mut bad = opening(&j);
    bad.extend(opening(&j));

    assert!(
        store
            .save_account(U, &snapshot(10_000_000), &bad, &[rec("e-1")])
            .await
            .is_err(),
        "повтор номера обязан падать"
    );
    assert_eq!(
        store.outbox_backlog().await.unwrap(),
        0,
        "событие не существует отдельно от денег"
    );
}

#[tokio::test]
#[ignore = "нужен живой PostgreSQL: TEST_DATABASE_URL"]
async fn envelope_survives_a_restart() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        panic!("TEST_DATABASE_URL не задан — живой прогон НЕ выполнен, это не «зелено»");
    };
    let store = persistence::PgStore::connect(&url)
        .await
        .expect("связь с БД");
    store.init().await.expect("схема");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&url)
        .await
        .expect("пул для подготовки");
    sqlx::query("DELETE FROM bus_outbox WHERE event_id LIKE 'live-%'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE account_events DISABLE TRIGGER account_events_no_update")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM account_events WHERE user_id = $1")
        .bind(U.0 as i64)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE account_events ENABLE TRIGGER account_events_no_update")
        .execute(&pool)
        .await
        .unwrap();

    let j = Journal::new(10_000_000);
    store
        .save_account(U, &snapshot(10_000_000), &opening(&j), &[rec("live-1")])
        .await
        .expect("запись");

    drop(store);
    let reborn = persistence::PgStore::connect(&url)
        .await
        .expect("связь заново");
    let claimed = reborn.claim_outbox(10, 0).await.expect("выборка");
    assert!(
        claimed.iter().any(|c| c.event_id == "live-1"),
        "конверт обязан пережить перезапуск — ради этого очередь и переехала из памяти"
    );

    let row = claimed.iter().find(|c| c.event_id == "live-1").unwrap();
    reborn
        .mark_failed(row.id, 3, 125_000, "проверка записи отказа")
        .await
        .expect("отказ записан");
    let (attempts, err): (i32, Option<String>) =
        sqlx::query_as("SELECT attempts, last_error FROM bus_outbox WHERE event_id = 'live-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(attempts, 3);
    assert_eq!(err.as_deref(), Some("проверка записи отказа"));
    assert!(
        reborn.claim_outbox(10, 0).await.unwrap().is_empty(),
        "до срока следующей попытки строку не берут и в БД"
    );

    reborn.mark_published(row.id).await.expect("пометка");
    assert!(
        !reborn
            .claim_outbox(10, 0)
            .await
            .unwrap()
            .iter()
            .any(|c| c.event_id == "live-1"),
        "отправленное повторно не берут"
    );
}
