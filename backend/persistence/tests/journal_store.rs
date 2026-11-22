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
                profile: Default::default(),
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
                lev: 1,
            },
        ],
    );
    let mut j2 = j.clone();
    for e in all.clone() {
        j2.commit(e).unwrap();
    }
    all.extend(j2.seal(
        1_700_000_050_000,
        &op,
        &[
            EventKind::Deposited { amount: 250_000 },
            EventKind::TradingFreezeSet { frozen: true },
        ],
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

#[tokio::test]
async fn memory_store_round_trips_the_journal() {
    let store = MemoryStore::new();
    let j = Journal::new(10_000_000);
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

    let rebuilt = Journal::load(10_000_000, lane).expect("цепочка обязана сойтись");
    assert_eq!(rebuilt.replay().balance, 10_250_000);
    assert!(
        rebuilt.state().trading_frozen,
        "заморозка поднялась из хранилища"
    );
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

#[tokio::test]
async fn memory_store_refuses_to_overwrite_or_skip() {
    let store = MemoryStore::new();
    let j = Journal::new(10_000_000);
    let events = trade_events(&j);
    store
        .save_account(U, &snapshot(), &events, &[])
        .await
        .unwrap();

    let again = store.save_account(U, &snapshot(), &events, &[]).await;
    assert!(again.is_err(), "повтор номера обязан быть отказом");

    let gap = Journal::load(10_000_000, events.clone()).unwrap();
    let mut next = gap.seal(
        1_700_000_060_000,
        &Origin::Client,
        &[
            EventKind::Withdrawn { amount: 1 },
            EventKind::Withdrawn { amount: 2 },
        ],
    );
    next.remove(0);
    assert!(
        store
            .save_account(U, &snapshot(), &next, &[])
            .await
            .is_err(),
        "пропуск номера обязан быть отказом"
    );
}

#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL=postgres://... cargo test -p persistence -- --ignored"]
async fn postgres_journal_is_append_only_at_the_database_level() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL не задан — тест запущен без базы, которую он проверяет");
    let store = persistence::PgStore::connect(&url)
        .await
        .expect("подключение");
    store.init().await.expect("DDL");

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

    let j = Journal::new(10_000_000);
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
    Journal::load(10_000_000, lane).expect("цепочка обязана сойтись после круга через БД");

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

    assert!(
        store
            .save_account(U, &snapshot(), &events, &[])
            .await
            .is_err(),
        "повтор номера обязан падать"
    );

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

#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL"]
async fn a_row_written_in_the_old_shape_still_loads() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        panic!("TEST_DATABASE_URL не задан — живой прогон НЕ выполнен, это не «зелено»");
    };
    const OLD: &str = "position_closed id=1 exit=240053 cause=command";
    const AT: i64 = 1_700_000_000_000;
    const SIGNED_BACK_THEN: &str = "cb7a4435a2bf0a5f9c3f3b48cca93ae0";
    let legacy = UserId(4242);

    let store = persistence::PgStore::connect(&url)
        .await
        .expect("связь с БД");
    store.init().await.expect("схема");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&url)
        .await
        .expect("пул");

    sqlx::query("ALTER TABLE account_events DISABLE TRIGGER account_events_no_update")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM account_events WHERE user_id = $1")
        .bind(legacy.0 as i64)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE account_events ENABLE TRIGGER account_events_no_update")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM accounts WHERE user_id = $1")
        .bind(legacy.0 as i64)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("INSERT INTO accounts (user_id, balance, next_id, model) VALUES ($1, 0, 0, 0)")
        .bind(legacy.0 as i64)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO account_events (user_id, seq, at_ms, origin, kind, prev_hash, hash)
         VALUES ($1, 1, $2, 'client', $3, repeat('0', 32), $4)",
    )
    .bind(legacy.0 as i64)
    .bind(AT)
    .bind(OLD)
    .bind(SIGNED_BACK_THEN)
    .execute(&pool)
    .await
    .unwrap();

    let loaded = store.load_all().await.expect("подъём");

    let snap = loaded
        .accounts
        .iter()
        .find(|(u, _)| *u == legacy)
        .expect("счёт обязан подняться")
        .1
        .clone();
    assert_eq!(snap.profile.kind, broker::AccountKind::Demo);
    assert_eq!(snap.profile.origin, broker::AccountOrigin::Registration);
    assert_eq!(snap.profile.currency, broker::Currency::USD);
    assert_eq!(
        snap.profile.owner.0, legacy.0,
        "владелец заполнен из user_id — миграции потом не потребуется"
    );

    let lane = loaded
        .journals
        .into_iter()
        .find(|(u, _)| *u == legacy)
        .expect("лента старого счёта обязана подняться")
        .1;
    assert_eq!(lane.len(), 1);
    assert_eq!(
        lane[0].canon(),
        OLD,
        "канон обязан приехать из базы как есть, а не пересобраться"
    );

    Journal::load(0, lane).expect(
        "журнал со старой записью обязан сойтись: иначе выкатка отправит в карантин все счета",
    );
}

#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL"]
async fn an_unreadable_account_kind_is_refused() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        panic!("TEST_DATABASE_URL не задан — живой прогон НЕ выполнен, это не «зелено»");
    };
    let victim = UserId(4343);
    let store = persistence::PgStore::connect(&url)
        .await
        .expect("связь с БД");
    store.init().await.expect("схема");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&url)
        .await
        .expect("пул");

    sqlx::query("DELETE FROM accounts WHERE user_id = $1")
        .bind(victim.0 as i64)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO accounts (user_id, balance, next_id, model, kind) VALUES ($1, 0, 0, 0, 'margin')",
    )
    .bind(victim.0 as i64)
    .execute(&pool)
    .await
    .unwrap();

    let err = store
        .load_all()
        .await
        .expect_err("счёт с непонятным родом обязан быть отказом, а не демо-счётом");
    let text = err.to_string();
    assert!(
        text.contains("род") && text.contains("margin"),
        "отказ обязан называть, что именно не прочиталось: {text}"
    );

    sqlx::query("DELETE FROM accounts WHERE user_id = $1")
        .bind(victim.0 as i64)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL"]
async fn the_storage_refuses_to_change_the_currency() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        panic!("TEST_DATABASE_URL не задан — живой прогон НЕ выполнен, это не «зелено»");
    };
    let acc = UserId(4444);
    let store = persistence::PgStore::connect(&url)
        .await
        .expect("связь с БД");
    store.init().await.expect("схема");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&url)
        .await
        .expect("пул");

    sqlx::query("DELETE FROM accounts WHERE user_id = $1")
        .bind(acc.0 as i64)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO accounts (user_id, balance, next_id, model, owner_user_id) VALUES ($1, 0, 0, 0, $1)",
    )
    .bind(acc.0 as i64)
    .execute(&pool)
    .await
    .unwrap();

    for (what, sql) in [
        (
            "валюта",
            "UPDATE accounts SET currency = 'JPY' WHERE user_id = $1",
        ),
        (
            "точность",
            "UPDATE accounts SET currency_minor_units = 0 WHERE user_id = $1",
        ),
        (
            "род",
            "UPDATE accounts SET kind = 'real' WHERE user_id = $1",
        ),
        (
            "владелец",
            "UPDATE accounts SET owner_user_id = 1 WHERE user_id = $1",
        ),
    ] {
        let res = sqlx::query(sql).bind(acc.0 as i64).execute(&pool).await;
        assert!(
            res.is_err(),
            "{what}: база обязана ОТКАЗАТЬ, а не тихо поправить"
        );
    }

    let fresh = UserId(4445);
    sqlx::query("DELETE FROM accounts WHERE user_id = $1")
        .bind(fresh.0 as i64)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO accounts (user_id, balance, next_id, model) VALUES ($1, 0, 0, 0)")
        .bind(fresh.0 as i64)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE accounts SET owner_user_id = $1 WHERE user_id = $1 AND owner_user_id IS NULL",
    )
    .bind(fresh.0 as i64)
    .execute(&pool)
    .await
    .expect("достройка пустого владельца обязана проходить");
    sqlx::query("DELETE FROM accounts WHERE user_id = $1")
        .bind(fresh.0 as i64)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("UPDATE accounts SET balance = 123 WHERE user_id = $1")
        .bind(acc.0 as i64)
        .execute(&pool)
        .await
        .expect("остаток — не профиль, его менять можно");

    sqlx::query("DELETE FROM accounts WHERE user_id = $1")
        .bind(acc.0 as i64)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL=postgres://... cargo test -p persistence -- --ignored"]
async fn postgres_round_trips_the_position_leverage() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL не задан — тест запущен без базы, которую он проверяет");
    let store = persistence::PgStore::connect(&url)
        .await
        .expect("подключение");
    store.init().await.expect("DDL");

    const V: UserId = UserId(78);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    for t in ["positions", "accounts"] {
        sqlx::query(&format!("DELETE FROM {t} WHERE user_id = $1"))
            .bind(V.0 as i64)
            .execute(&pool)
            .await
            .unwrap();
    }

    let pos = broker::Position {
        id: 1,
        instrument: 2,
        side: PosSide::Long,
        qty: 1000,
        entry: 239_883,
        pd: 2,
        qd: 4,
        margin: 239,
        sl: None,
        tp: None,
        leverage: 100,
        fees: Default::default(),
    };
    let snap = AccountSnapshot {
        balance: 10_000_000,
        next_id: 1,
        positions: vec![pos.clone()],
        ..Default::default()
    };
    assert_eq!(
        broker::check_snapshot(&snap),
        vec![],
        "предпосылка: до записи слепок здоров"
    );

    store
        .save_account(V, &snap, &[], &[])
        .await
        .expect("запись в БД");
    let loaded = store.load_all().await.expect("подъём из БД");
    let back = loaded
        .accounts
        .into_iter()
        .find(|(id, _)| *id == V)
        .expect("счёт поднялся")
        .1;

    assert_eq!(
        back.positions.first().expect("позиция поднялась").leverage,
        100,
        "плечо позиции обязано пережить хранилище"
    );
    assert_eq!(
        broker::check_snapshot(&back),
        vec![],
        "поднятый счёт здоров: маржа отвечает плечу, которое доехало обратно"
    );
}

#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL=postgres://... cargo test -p persistence -- --ignored"]
async fn postgres_round_trips_the_account_leverage() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL не задан — тест запущен без базы, которую он проверяет");
    let store = persistence::PgStore::connect(&url)
        .await
        .expect("подключение");
    store.init().await.expect("DDL");

    const W: UserId = UserId(79);
    const N: UserId = UserId(80);
    let classes: broker::ClassLeverage = [
        (broker::LevClass::Crypto, 5),
        (broker::LevClass::Forex, 100),
    ]
    .into_iter()
    .collect();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE account_events DISABLE TRIGGER account_events_no_update")
        .execute(&pool)
        .await
        .unwrap();
    for u in [W, N] {
        for t in ["positions", "pendings", "accounts", "account_events"] {
            sqlx::query(&format!("DELETE FROM {t} WHERE user_id = $1"))
                .bind(u.0 as i64)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    sqlx::query("ALTER TABLE account_events ENABLE TRIGGER account_events_no_update")
        .execute(&pool)
        .await
        .unwrap();

    let j = Journal::new(10_000_000);
    let op = Origin::Operator(Operator::new("risk@site", "портфель Pro", "ext:adm-1").unwrap());
    let mut events = j.seal(
        1_700_000_000_000,
        &Origin::System,
        &[EventKind::AccountOpened {
            balance: 10_000_000,
            profile: Default::default(),
        }],
    );
    let mut j2 = j.clone();
    for e in events.clone() {
        j2.commit(e).unwrap();
    }
    let tail = j2.seal(
        1_700_000_001_000,
        &op,
        &[
            EventKind::LeverageSet { lev: 50 },
            EventKind::LeverageClassesSet {
                classes: classes.clone(),
            },
            EventKind::PendingPlaced {
                id: 1,
                instrument: 1,
                side: PosSide::Long,
                qty: 10,
                price: 4_900,
                pd: 2,
                qd: 3,
                sl: None,
                tp: None,
            },
            EventKind::PendingRejected {
                id: 1,
                exec: Some(4_800),
                reason: broker::journal::RejectReason::AccountFrozen,
            },
        ],
    );
    events.extend(tail);
    let snap = AccountSnapshot {
        balance: 10_000_000,
        next_id: 1,
        leverage: Some(50),
        class_leverage: classes.clone(),
        ..Default::default()
    };
    store
        .save_account(W, &snap, &events, &[])
        .await
        .expect("запись в БД");
    let plain = AccountSnapshot {
        balance: 10_000_000,
        ..Default::default()
    };
    store
        .save_account(N, &plain, &[], &[])
        .await
        .expect("запись в БД");

    let loaded = store.load_all().await.expect("подъём из БД");
    let back = |u: UserId| {
        loaded
            .accounts
            .iter()
            .find(|(id, _)| *id == u)
            .expect("счёт поднялся")
            .1
            .clone()
    };
    assert_eq!(back(W).leverage, Some(50), "плечо счёта пережило хранилище");
    assert_eq!(back(W).class_leverage, classes, "плечо по категориям тоже");
    assert!(back(N).class_leverage.is_empty());
    assert_eq!(
        back(N).leverage,
        None,
        "не назначали — остаётся не назначенным"
    );

    let lane = loaded
        .journals
        .into_iter()
        .find(|(id, _)| *id == W)
        .expect("лента поднялась")
        .1;
    let journal = Journal::load(10_000_000, lane).expect("цепочка цела");
    assert_eq!(journal.state().leverage, Some(50));
    assert_eq!(journal.state().class_leverage, classes);
    assert!(
        broker::journal::audit(&journal, &back(W)).ok(),
        "лента и слепок сходятся после подъёма"
    );
}

#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL=postgres://... cargo test -p persistence -- --ignored"]
async fn postgres_round_trips_the_account_fees() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL не задан — тест запущен без базы, которую он проверяет");
    let store = persistence::PgStore::connect(&url)
        .await
        .expect("подключение");
    store.init().await.expect("DDL");

    const W: UserId = UserId(81);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE account_events DISABLE TRIGGER account_events_no_update")
        .execute(&pool)
        .await
        .unwrap();
    for t in [
        "positions",
        "pendings",
        "closed_deals",
        "accounts",
        "account_events",
    ] {
        sqlx::query(&format!("DELETE FROM {t} WHERE user_id = $1"))
            .bind(W.0 as i64)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("ALTER TABLE account_events ENABLE TRIGGER account_events_no_update")
        .execute(&pool)
        .await
        .unwrap();

    let mut b = broker::Broker::new(1_000_000_000, 20);
    assert!(b.open_account(W));
    let mut j = Journal::new(b.start_balance());
    let mut all = Vec::new();
    let mut step = |b: &mut broker::Broker, j: &mut Journal, origin: &Origin| {
        let kinds = b.take_events(W);
        let sealed = j.seal(1_700_000_000_000, origin, &kinds);
        for e in sealed.clone() {
            j.commit(e).unwrap();
        }
        all.extend(sealed);
    };
    step(&mut b, &mut j, &Origin::System);
    let op = Origin::Operator(Operator::new("risk@site", "портфель Pro", "ext:adm-9").unwrap());
    b.set_account_fees(
        W,
        broker::Fees {
            markup_bps: 10,
            commission_bps: 7,
        },
    )
    .unwrap();
    step(&mut b, &mut j, &op);
    let marks = std::collections::HashMap::from([(1u32, 5_000_000i64)]);
    let a = b
        .open(
            W,
            &marks,
            1,
            PosSide::Long,
            1000,
            5_000_000,
            2,
            3,
            None,
            None,
        )
        .unwrap();
    step(&mut b, &mut j, &Origin::Client);
    b.open(
        W,
        &marks,
        1,
        PosSide::Short,
        1000,
        5_000_000,
        2,
        3,
        None,
        None,
    )
    .unwrap();
    step(&mut b, &mut j, &Origin::Client);
    b.close(W, a, 5_100_000).unwrap();
    step(&mut b, &mut j, &Origin::Client);

    let snap = b.snapshot(W);
    assert!(
        broker::journal::audit(&j, &snap).ok(),
        "до хранилища сходится"
    );
    store
        .save_account(W, &snap, &all, &[])
        .await
        .expect("запись в БД");

    let loaded = store.load_all().await.expect("подъём из БД");
    let back = loaded
        .accounts
        .iter()
        .find(|(id, _)| *id == W)
        .expect("счёт поднялся")
        .1
        .clone();
    assert_eq!(back.fees, snap.fees, "тариф счёта пережил хранилище");
    assert_eq!(back.positions.len(), 1);
    assert_eq!(
        back.positions[0].fees, snap.positions[0].fees,
        "снимок тарифа позиции"
    );
    assert!(back.positions[0].fees.commission > 0);
    assert_eq!(
        back.closed[0].fees, snap.closed[0].fees,
        "доход площадки со сделки"
    );
    assert!(back.closed[0].fees.commission > 0 && back.closed[0].fees.markup > 0);
    assert_eq!(back.closed[0].fees.market_exit, Some(5_100_000));

    let lane = loaded
        .journals
        .into_iter()
        .find(|(id, _)| *id == W)
        .expect("лента поднялась")
        .1;
    let journal = Journal::load(1_000_000_000, lane).expect("цепочка цела");
    assert!(
        broker::journal::audit(&journal, &back).ok(),
        "лента и слепок сходятся после подъёма: {}",
        broker::journal::audit(&journal, &back)
    );
    assert!(journal
        .events()
        .iter()
        .any(|e| matches!(e.kind(), EventKind::CommissionCharged { .. })));
}
