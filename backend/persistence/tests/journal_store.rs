//! Журнал в хранилище (TRM-13): он обязан работать и проверяться в **обоих** режимах.
//!
//! Без БД режим проверяется здесь и на каждом прогоне. С БД — живым прогоном против
//! настоящего PostgreSQL: тест помечен `#[ignore]`, потому что живой базы в обычном
//! прогоне нет, и он **виден в выводе как пропущенный**, а не притворяется зелёным.
//! Запуск: `TEST_DATABASE_URL=postgres://... cargo test -p persistence -- --ignored`.

use broker::journal::{EventKind, Journal, Operator, Origin};
use broker::{AccountSnapshot, PosSide};
use domain::account::UserId;
use persistence::{MemoryStore, Persistence};

const U: UserId = UserId(77);

fn trade_events(j: &Journal) -> Vec<broker::journal::AccountEvent> {
    let op = Origin::Operator(
        Operator::new("ops@platform", "сбой на стороне провайдера", "INC-42").unwrap(),
    );
    let mut all = j.seal(
        1_700_000_000_000,
        &Origin::Client,
        &[
            EventKind::AccountOpened {
                balance: 10_000_000,
            },
            EventKind::PositionOpened {
                id: 1,
                instrument: 3,
                side: PosSide::Short,
                qty: 500,
                entry: 40_000,
                pd: 2,
                qd: 3,
                sl: None,
                tp: Some(39_000),
            },
        ],
    );
    // Продолжаем ленту: ручная проводка обязана ездить в хранилище целиком — кто,
    // почему, по какому инциденту.
    let mut j2 = j.clone();
    for e in all.clone() {
        j2.commit(e).unwrap();
    }
    all.extend(j2.seal(
        1_700_000_050_000,
        &op,
        &[EventKind::Deposited { amount: 250_000 }],
    ));
    all
}

fn snapshot() -> AccountSnapshot {
    AccountSnapshot {
        balance: 10_250_000,
        next_id: 1,
        ..Default::default()
    }
}

// ============================ Режим без БД ==================================

#[tokio::test]
async fn memory_store_round_trips_the_journal() {
    let store = MemoryStore::new();
    let j = Journal::new(1, 10_000_000);
    let events = trade_events(&j);
    store
        .save_account(U, &snapshot(), &events, &[])
        .await
        .expect("запись");

    let loaded = store.load_all().await.expect("подъём");
    let (user, lane) = loaded
        .journals
        .into_iter()
        .next()
        .expect("журнал обязан подняться");
    assert_eq!(user, U);
    assert_eq!(lane, events, "поднялось ровно то, что положили");

    // И цепочка сходится после подъёма — иначе хранилище что-то потеряло по дороге.
    let rebuilt = Journal::load(1, 10_000_000, lane).expect("цепочка обязана сойтись");
    assert_eq!(rebuilt.replay().balance, 10_250_000);
    let op = rebuilt
        .events()
        .last()
        .unwrap()
        .origin()
        .operator()
        .expect("ручная проводка обязана остаться ручной");
    assert_eq!(
        (op.who(), op.reason(), op.incident()),
        ("ops@platform", "сбой на стороне провайдера", "INC-42")
    );
}

/// Поверх записанного не пишут. Номер, который уже занят, — отказ, а не замена:
/// журнал, в котором можно переписать запись, журналом не является.
#[tokio::test]
async fn memory_store_refuses_to_overwrite_or_skip() {
    let store = MemoryStore::new();
    let j = Journal::new(1, 10_000_000);
    let events = trade_events(&j);
    store
        .save_account(U, &snapshot(), &events, &[])
        .await
        .unwrap();

    let again = store.save_account(U, &snapshot(), &events, &[]).await;
    assert!(again.is_err(), "повтор номера обязан быть отказом");

    // И пропуск номера тоже: дыра в ленте — потерянная история.
    let gap = Journal::load(1, 10_000_000, events.clone()).unwrap();
    let mut next = gap.seal(
        1_700_000_060_000,
        &Origin::Client,
        &[
            EventKind::Withdrawn { amount: 1 },
            EventKind::Withdrawn { amount: 2 },
        ],
    );
    next.remove(0); // первую «потеряли» — вторая обязана не пройти
    assert!(
        store
            .save_account(U, &snapshot(), &next, &[])
            .await
            .is_err(),
        "пропуск номера обязан быть отказом"
    );
}

// ============================ Режим с живой БД ==============================

/// Живой прогон против PostgreSQL. Проверяется то, что без базы проверить нечем:
/// запись/подъём настоящим SQL и **неизменяемость на уровне БД** — журнал не может
/// изменить даже тот, кто в него пишет.
#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL=postgres://... cargo test -p persistence -- --ignored"]
async fn postgres_journal_is_append_only_at_the_database_level() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL не задан — тест запущен без базы, которую он проверяет");
    let store = persistence::PgStore::connect(&url)
        .await
        .expect("подключение");
    store.init().await.expect("DDL");

    // Чистим свой след: тест обязан быть повторяемым.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("DELETE FROM accounts WHERE user_id = $1")
        .bind(U.0 as i64)
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

    let j = Journal::new(1, 10_000_000);
    let events = trade_events(&j);
    store
        .save_account(U, &snapshot(), &events, &[])
        .await
        .expect("запись в БД");

    let loaded = store.load_all().await.expect("подъём из БД");
    let lane = loaded
        .journals
        .into_iter()
        .find(|(u, _)| *u == U)
        .expect("журнал обязан подняться")
        .1;
    assert_eq!(lane, events, "из БД поднялось ровно то, что записали");
    Journal::load(1, 10_000_000, lane).expect("цепочка обязана сойтись после круга через БД");

    // Неизменяемость — не на честном слове приложения: правка и удаление запрещены
    // самой базой.
    let upd = sqlx::query("UPDATE account_events SET kind = 'deposited amount=999999999' WHERE user_id = $1 AND seq = 1")
        .bind(U.0 as i64)
        .execute(&pool)
        .await;
    assert!(upd.is_err(), "БД обязана запретить правку записи журнала");
    let del = sqlx::query("DELETE FROM account_events WHERE user_id = $1 AND seq = 1")
        .bind(U.0 as i64)
        .execute(&pool)
        .await;
    assert!(del.is_err(), "БД обязана запретить удаление записи журнала");

    // Повтор номера тоже отказ — на уникальном ключе, а не на вежливости кода.
    assert!(
        store
            .save_account(U, &snapshot(), &events, &[])
            .await
            .is_err(),
        "повтор номера обязан падать"
    );

    // И после всех попыток лента цела.
    let after = store
        .load_all()
        .await
        .unwrap()
        .journals
        .into_iter()
        .find(|(u, _)| *u == U)
        .unwrap()
        .1;
    assert_eq!(
        after, events,
        "журнал обязан остаться ровно таким, каким записан"
    );
}
