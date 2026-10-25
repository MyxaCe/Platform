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

/// **Запись СТАРОГО вида, лежащая в базе, поднимается новым кодом** (TRM-44).
///
/// Это закрывающий критерий миграции в исполнимом виде: «сверка существующих
/// счетов после выкатки проходит». Проверить его можно только так — положив в
/// базу строку в том виде, в каком её писал прежний код, и подняв сегодняшним.
///
/// Обычные проверки этого не ловят **по построению**: они пишут через
/// `save_account`, то есть всегда текущим видом, и расхождения между «что
/// записано» и «что пересобирается» там не возникает. Локально база пустая, и
/// первый счёт заводится уже новым кодом — поэтому дефект невидим до миграции.
///
/// Отпечаток — литерал, посчитанный сторонней реализацией FNV-1a, а не нашим
/// `digest`: иначе проверка сравнивала бы подпись с собой.
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

    // Чистим прошлый прогон. Лента неизменяема триггером — снимаем его на время,
    // как это делает соседний тест: убираем СВОЙ мусор, а не чужую историю.
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

    // Пишем строку МИМО save_account — ровно так, как её положил бы прежний код.
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

    // ЗАКРЫВАЮЩИЙ КРИТЕРИЙ МИГРАЦИИ (TRM-44): строка счёта, заведённая БЕЗ
    // столбцов профиля, поднимается и получает профиль счетов, существовавших
    // до правки, — демо, розничный, доллар, регистрация. Столбцы добавлены
    // `ALTER … IF NOT EXISTS` с умолчаниями, и умолчания описывают историю, а
    // не удобство: другого рода счетов в терминале не было.
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

    Journal::load(1, 0, lane).expect(
        "журнал со старой записью обязан сойтись: иначе выкатка отправит в карантин все счета",
    );
}

/// **Непонятное значение в профиле счёта — отказ чтения, а не умолчание**
/// (TRM-44).
///
/// Род счёта решает, настоящие ли это деньги. Прочитать непонятное как «демо»
/// значило бы угадывать, чьими деньгами торгуют: счёт с испорченным или более
/// новым значением поднялся бы как бумажный и пустил бы к торговле без
/// проверки личности.
///
/// Проверяется только живой базой: подсунуть такое значение можно лишь в саму
/// таблицу — код его не напишет.
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

/// **Сменить валюту счёта нельзя — отказывает ХРАНИЛИЩЕ** (TRM-55, требование
/// владельца).
///
/// Отсутствие профиля в `DO UPDATE` защищает только от нашего кода; прямой
/// `UPDATE` по таблице его обходит. Запрет стоит там, где данные, — как у
/// журнала, закрытого от правок тем же способом.
///
/// Почему это не «не рекомендуется»: смена валюты у живого счёта означает, что
/// прошлые суммы посчитаны в одной наименьшей единице, а читаются в другой.
/// Деньги меняются в `10^n` раз, и ни одна проверка этого не заметит — число
/// осталось прежним.
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
    // Владельца задаём СРАЗУ: пустой столбец — это недостроенная строка, и его
    // заполнение разрешено намеренно (иначе сама миграция стала бы нарушением).
    // Чтобы проверять запрет на СМЕНУ, начинать надо с заполненного.
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

    // А достройка пустого владельца — РАЗРЕШЕНА, и это не поблажка: столбца
    // раньше не существовало, и `UPDATE … WHERE owner_user_id IS NULL` в самой
    // схеме — это она и есть. Запрети её — и миграция станет нарушением.
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

    // А деньги меняться обязаны: запрет на профиль не должен запереть счёт.
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
