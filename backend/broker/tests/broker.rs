//! Тесты бумажного брокера: маржа, P&L, закрытие, лимитные ордера, SL/TP, история.

use std::collections::HashMap;

use broker::journal::{CloseCause, EventKind};
use broker::{Broker, BrokerError, PosSide};
use domain::account::UserId;

/// Пустой справочник цен: без цен нереализованный результат равен нулю, то есть
/// `equity == balance`, и проверки продолжают проверять ровно то, что проверяли
/// до появления единого определения свободной маржи (TRM-41).
fn no_marks() -> std::collections::HashMap<u32, i64> {
    std::collections::HashMap::new()
}

const ALICE: UserId = UserId(1);
const PD: u8 = 2;
const QD: u8 = 5;
// BTC pd=2 qd=5: цена 60000.00 → raw 6_000_000; объём 0.01 → raw 1000; нотионал = $600 = 60000 центов.

fn marks(inst: u32, price: i64) -> HashMap<u32, i64> {
    HashMap::from([(inst, price)])
}
fn open(b: &mut Broker, side: PosSide, price: i64) -> u64 {
    b.open(ALICE, &no_marks(), 1, side, 1000, price, PD, QD, None, None)
        .unwrap()
}

// ---- Базовое ---------------------------------------------------------------

#[test]
fn open_locks_margin() {
    let mut b = Broker::new(1_000_000, 1);
    open(&mut b, PosSide::Long, 6_000_000);
    assert_eq!(b.used_margin(ALICE), 60_000);
    assert_eq!(b.free_margin(ALICE, &no_marks()), 940_000);
    assert_eq!(b.balance(ALICE), 1_000_000);
}

#[test]
fn insufficient_margin_is_rejected() {
    let mut b = Broker::new(50_000, 1);
    assert_eq!(
        b.open(
            ALICE,
            &no_marks(),
            1,
            PosSide::Long,
            1000,
            6_000_000,
            PD,
            QD,
            None,
            None
        ),
        Err(BrokerError::InsufficientMargin)
    );
}

#[test]
fn long_pnl_and_short_pnl() {
    let mut b = Broker::new(1_000_000, 1);
    open(&mut b, PosSide::Long, 6_000_000);
    assert_eq!(b.open_pnl(ALICE, &marks(1, 6_100_000)), 1000); // +$10
    let mut b2 = Broker::new(1_000_000, 1);
    b2.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Short,
        1000,
        6_000_000,
        PD,
        QD,
        None,
        None,
    )
    .unwrap();
    assert_eq!(b2.open_pnl(ALICE, &marks(1, 5_900_000)), 1000); // short в плюсе на падении
}

#[test]
fn close_realizes_pnl_and_records_history() {
    let mut b = Broker::new(1_000_000, 1);
    let id = open(&mut b, PosSide::Long, 6_000_000);
    let pnl = b.close(ALICE, id, 6_100_000).unwrap();
    assert_eq!(pnl, 1000);
    assert_eq!(b.balance(ALICE), 1_001_000);
    assert!(b.positions(ALICE).is_empty());
    let closed = b.closed_deals(ALICE);
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].pnl, 1000);
    assert_eq!(closed[0].exit, 6_100_000);
}

#[test]
fn losing_trade_reduces_balance() {
    let mut b = Broker::new(1_000_000, 1);
    let id = open(&mut b, PosSide::Long, 6_000_000);
    b.close(ALICE, id, 5_900_000).unwrap();
    assert_eq!(b.balance(ALICE), 999_000);
}

// ---- Лимитные ордера -------------------------------------------------------

#[test]
fn pending_buy_triggers_when_price_drops() {
    let mut b = Broker::new(1_000_000, 1);
    let pid = b.place_pending(ALICE, 1, PosSide::Long, 1000, 5_900_000, PD, QD, None, None);
    assert_eq!(b.pendings(ALICE).len(), 1);
    b.check(&marks(1, 6_000_000), false); // цена выше лимита — не сработал
    assert_eq!(b.pendings(ALICE).len(), 1);
    b.check(&marks(1, 5_900_000), false); // цена дошла до лимита — сработал
    assert!(b.pendings(ALICE).is_empty());
    assert_eq!(b.positions(ALICE).len(), 1);
    let _ = pid;
}

#[test]
fn cancel_pending() {
    let mut b = Broker::new(1_000_000, 1);
    let pid = b.place_pending(ALICE, 1, PosSide::Long, 1000, 5_900_000, PD, QD, None, None);
    assert_eq!(b.cancel_pending(ALICE, pid), Ok(()));
    assert!(b.pendings(ALICE).is_empty());
    assert_eq!(
        b.cancel_pending(ALICE, 999),
        Err(BrokerError::UnknownPending)
    );
}

// ---- SL/TP -----------------------------------------------------------------

#[test]
fn stop_loss_closes_position() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Long,
        1000,
        6_000_000,
        PD,
        QD,
        Some(5_950_000),
        Some(6_100_000),
    )
    .unwrap();
    b.check(&marks(1, 5_990_000), false); // между SL и TP — не трогаем
    assert_eq!(b.positions(ALICE).len(), 1);
    b.check(&marks(1, 5_950_000), false); // достигли SL — закрыли
    assert!(b.positions(ALICE).is_empty());
    let closed = b.closed_deals(ALICE);
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].exit, 5_950_000);
    assert!(closed[0].pnl < 0);
}

#[test]
fn take_profit_closes_position() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Long,
        1000,
        6_000_000,
        PD,
        QD,
        Some(5_950_000),
        Some(6_100_000),
    )
    .unwrap();
    b.check(&marks(1, 6_100_000), false); // достигли TP
    assert!(b.positions(ALICE).is_empty());
    assert_eq!(b.closed_deals(ALICE)[0].pnl, 1000);
}

// ---- Персистентность (ADR-016) --------------------------------------------

#[test]
fn check_reports_only_changed_accounts() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Long,
        1000,
        6_000_000,
        PD,
        QD,
        Some(5_950_000),
        None,
    )
    .unwrap();

    // Цена движется, но ничего не сработало — сохранять нечего.
    assert!(b.check(&marks(1, 5_990_000), false).is_empty());

    // Достигли SL — счёт изменился и попал в список на сохранение.
    assert_eq!(b.check(&marks(1, 5_950_000), false), vec![ALICE]);

    // Позиций больше нет — следующий тик снова пустой.
    assert!(b.check(&marks(1, 5_900_000), false).is_empty());
}

#[test]
fn check_reports_account_on_pending_fill() {
    let mut b = Broker::new(1_000_000, 1);
    b.place_pending(ALICE, 1, PosSide::Long, 1000, 5_900_000, PD, QD, None, None);
    assert!(b.check(&marks(1, 6_000_000), false).is_empty()); // выше лимита — не сработал
    assert_eq!(b.check(&marks(1, 5_900_000), false), vec![ALICE]); // сработал
    assert_eq!(b.positions(ALICE).len(), 1);
}

#[test]
fn snapshot_restore_roundtrip() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Long,
        1000,
        6_000_000,
        PD,
        QD,
        Some(5_950_000),
        None,
    )
    .unwrap();
    b.place_pending(ALICE, 2, PosSide::Short, 500, 7_000_000, PD, QD, None, None);
    b.check(&marks(1, 5_950_000), false); // закрыли по SL → появилась история
    let snap = b.snapshot(ALICE);
    assert_eq!(snap.pendings.len(), 1);
    assert_eq!(snap.closed.len(), 1);

    // Новый брокер (как после перезапуска) восстанавливает счёт из слепка.
    let mut fresh = Broker::new(1_000_000, 1);
    fresh.restore(ALICE, snap);
    assert_eq!(fresh.balance(ALICE), b.balance(ALICE));
    assert_eq!(fresh.pendings(ALICE).len(), 1);
    assert_eq!(fresh.closed_deals(ALICE).len(), 1);
    assert_eq!(fresh.users(), vec![ALICE]);

    // next_id восстановлен: новый ордер не должен переиспользовать занятый id.
    let used: Vec<u64> = fresh.pendings(ALICE).iter().map(|p| p.id).collect();
    let new_id = fresh.place_pending(ALICE, 3, PosSide::Long, 100, 1_000_000, PD, QD, None, None);
    assert!(!used.contains(&new_id));
}

#[test]
fn snapshot_of_unknown_user_is_start_balance() {
    let b = Broker::new(1_000_000, 1);
    let snap = b.snapshot(ALICE);
    assert_eq!(snap.balance, 1_000_000);
    assert!(snap.positions.is_empty() && snap.closed.is_empty());
}

// ---- Пополнение и вывод ----------------------------------------------------

#[test]
fn deposit_increases_balance() {
    let mut b = Broker::new(1_000_000, 1);
    assert_eq!(b.balance(ALICE), 1_000_000);
    assert_eq!(b.deposit(ALICE, 500_000), 1_500_000);
    assert_eq!(b.balance(ALICE), 1_500_000);
}

#[test]
fn withdraw_decreases_balance() {
    let mut b = Broker::new(1_000_000, 1);
    assert_eq!(b.withdraw(ALICE, 400_000, &no_marks()).unwrap(), 600_000);
    assert_eq!(b.balance(ALICE), 600_000);
}

#[test]
fn cannot_withdraw_more_than_free() {
    let mut b = Broker::new(1_000_000, 1);
    assert_eq!(
        b.withdraw(ALICE, 1_000_001, &no_marks()),
        Err(BrokerError::InsufficientFunds)
    );
    assert_eq!(
        b.balance(ALICE),
        1_000_000,
        "неудачный вывод не меняет баланс"
    );
}

#[test]
fn margin_locks_funds_from_withdrawal() {
    let mut b = Broker::new(1_000_000, 1);
    // Открываем позицию: нотионал = маржа при leverage=1 → часть баланса занята.
    b.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Long,
        1000,
        6_000_000,
        PD,
        QD,
        None,
        None,
    )
    .unwrap();
    let free = b.free_margin(ALICE, &no_marks());
    assert!(free < 1_000_000, "часть средств ушла под маржу");
    // Больше свободного вывести нельзя.
    assert_eq!(
        b.withdraw(ALICE, free + 1, &no_marks()),
        Err(BrokerError::InsufficientFunds)
    );
    // Ровно свободное — можно.
    assert!(b.withdraw(ALICE, free, &no_marks()).is_ok());
}
// ---- Разрыв ряда, проскальзывание и правда в журнале (TRM-19, TRM-31) -----

/// Рынок на разрыве перепрыгнул стоп. Исполняем по НАБЛЮДЁННОЙ цене (Р-178):
/// гарантия уровня означала бы, что разницу платит брокер — на каждом скачке и
/// без предела. В журнал идут обе цены, иначе проскальзывание потом не из чего
/// восстановить.
#[test]
fn gap_close_executes_at_the_observed_price_and_records_the_level() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Long,
        1000,
        6_000_000,
        PD,
        QD,
        Some(5_950_000),
        None,
    )
    .unwrap();

    assert_eq!(
        b.check(&marks(1, 5_400_000), true),
        vec![ALICE],
        "стоп обязан сработать и на разрыве"
    );

    let closed = b.closed_deals(ALICE);
    assert_eq!(
        closed[0].exit, 5_400_000,
        "исполнено по рынку, а не по уровню"
    );

    match b
        .take_events(ALICE)
        .last()
        .expect("закрытие обязано быть в событиях")
    {
        EventKind::PositionClosed {
            exit,
            level,
            across_gap,
            cause,
            ..
        } => {
            assert_eq!(*exit, 5_400_000, "в журнале цена исполнения");
            assert_eq!(*level, Some(5_950_000), "и уровень, который ставил человек");
            assert!(*across_gap, "факт разрыва обязан быть виден");
            assert_eq!(*cause, CloseCause::StopLoss);
        }
        other => panic!("ожидалось закрытие позиции, а не {other:?}"),
    }
}

/// Проскальзывание — это РАЗНИЦА, и она должна быть больше у большего разрыва.
/// Числа тут не эталон, а следствие: проверяется свойство, а не сегодняшний
/// результат.
#[test]
fn bigger_gap_costs_the_client_more() {
    let loss_at = |mark: i64| {
        let mut b = Broker::new(1_000_000, 1);
        b.open(
            ALICE,
            &no_marks(),
            1,
            PosSide::Long,
            1000,
            6_000_000,
            PD,
            QD,
            Some(5_950_000),
            None,
        )
        .unwrap();
        b.check(&marks(1, mark), true);
        b.closed_deals(ALICE)[0].pnl
    };
    let small = loss_at(5_940_000);
    let big = loss_at(5_400_000);
    assert!(
        big < small,
        "чем дальше рынок перепрыгнул стоп, тем больше потерял клиент: {big} против {small}"
    );
}

/// Когда рынок дошёл ровно до уровня, проскальзывания нет и разрыв не
/// объявляется. Иначе признак стоял бы всегда и его перестали бы читать.
#[test]
fn quiet_close_has_no_slippage_and_is_not_marked_as_a_gap() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Long,
        1000,
        6_000_000,
        PD,
        QD,
        Some(5_950_000),
        None,
    )
    .unwrap();
    b.check(&marks(1, 5_950_000), false);

    match b.take_events(ALICE).last().expect("закрытие") {
        EventKind::PositionClosed {
            exit,
            level,
            across_gap,
            ..
        } => {
            assert_eq!(
                *level,
                Some(*exit),
                "исполнилось ровно по поставленному — проскальзывания нет"
            );
            assert!(!*across_gap, "спокойное закрытие разрывом не зовётся");
        }
        other => panic!("ожидалось закрытие позиции, а не {other:?}"),
    }
}

/// Тейк-профит на разрыве проскальзывает В ПОЛЬЗУ клиента: рынок перепрыгнул
/// уровень вверх, и продано выше, чем он просил. Правило одно и то же —
/// исполняется то, что на рынке; «в нашу пользу» исключением не является.
#[test]
fn take_profit_slips_in_the_clients_favour() {
    // Сравниваем с тем, что дал бы УРОВЕНЬ, а не с числом: число — это
    // сегодняшняя шкала, а утверждение здесь про направление выгоды.
    let pnl_at = |mark: i64| {
        let mut b = Broker::new(1_000_000, 1);
        b.open(
            ALICE,
            &no_marks(),
            1,
            PosSide::Long,
            1000,
            6_000_000,
            PD,
            QD,
            None,
            Some(6_100_000),
        )
        .unwrap();
        b.check(&marks(1, mark), true);
        let closed = b.closed_deals(ALICE);
        (closed[0].exit, closed[0].pnl)
    };

    let (exit_at_level, pnl_at_level) = pnl_at(6_100_000);
    let (exit_gapped, pnl_gapped) = pnl_at(6_300_000);

    assert_eq!(
        exit_at_level, 6_100_000,
        "рынок дошёл ровно до тейка — исполнение совпало с уровнем"
    );
    assert_eq!(
        exit_gapped, 6_300_000,
        "рынок перепрыгнул тейк — исполнено по рынку, а не по уровню"
    );
    assert!(
        pnl_gapped > pnl_at_level,
        "на разрыве вверх клиент обязан получить БОЛЬШЕ, чем по уровню: {pnl_gapped} против {pnl_at_level}"
    );
}

/// Закрытие по команде человека: уровня там нет вовсе, и разрывом оно не
/// метится никогда — исполняется ровно та цена, которую показали.
#[test]
fn manual_close_names_no_level() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Long,
        1000,
        6_000_000,
        PD,
        QD,
        None,
        None,
    )
    .unwrap();
    let id = b.positions(ALICE)[0].id;
    b.close(ALICE, id, 6_010_000).unwrap();

    match b.take_events(ALICE).last().expect("закрытие") {
        EventKind::PositionClosed {
            level,
            across_gap,
            cause,
            ..
        } => {
            assert_eq!(*level, None);
            assert!(!*across_gap);
            assert_eq!(*cause, CloseCause::Command);
        }
        other => panic!("ожидалось закрытие позиции, а не {other:?}"),
    }
}

/// Лимитный ордер на разрыве исполняется НЕ ХУЖЕ своего уровня: покупка
/// срабатывает, когда рынок опустился до уровня или ниже. Позиция обязана
/// открыться по рыночной цене, а не по заявленной.
#[test]
fn pending_fill_on_a_gap_opens_at_the_market_not_at_the_level() {
    let mut b = Broker::new(1_000_000, 1);
    b.place_pending(ALICE, 1, PosSide::Long, 100, 5_900_000, PD, QD, None, None);
    b.check(&marks(1, 5_300_000), true);

    let pos = b.positions(ALICE);
    assert_eq!(pos.len(), 1, "ордер обязан был открыть позицию");
    assert_eq!(
        pos[0].entry, 5_300_000,
        "вход по рынку: клиент купил дешевле, чем просил"
    );

    let ev = b.take_events(ALICE);
    let fill = ev
        .iter()
        .find(|k| matches!(k, EventKind::PendingTriggered { .. }))
        .expect("сработавший ордер обязан быть в событиях");
    match fill {
        EventKind::PendingTriggered {
            price,
            exec,
            across_gap,
            ..
        } => {
            assert_eq!(*price, 5_900_000, "уровень ордера");
            assert_eq!(*exec, Some(5_300_000), "и цена, по которой он исполнен");
            assert!(*across_gap);
        }
        other => panic!("ожидался сработавший ордер, а не {other:?}"),
    }
}

/// Закрытая сделка несёт наружу **обе** цены и признак разрыва (TRM-38).
///
/// Витрина показывает проскальзывание, а восстановить его из одной `exit`
/// нельзя. В журнале обе величины были и раньше, но журнал читает владелец
/// счёта, а проекции — другие потребители: одна величина, записанная в одном
/// месте, не становится доступной во всех.
#[test]
fn closed_deal_carries_the_level_and_the_gap_outward() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Long,
        1000,
        6_000_000,
        PD,
        QD,
        Some(5_950_000),
        None,
    )
    .unwrap();
    b.check(&marks(1, 5_400_000), true);

    let d = &b.closed_deals(ALICE)[0];
    assert_eq!(d.exit, 5_400_000, "цена исполнения");
    assert_eq!(
        d.level,
        Some(5_950_000),
        "и поставленный уровень — иначе проскальзывание не из чего считать"
    );
    assert_eq!(
        d.across_gap,
        Some(true),
        "и признак разрыва, объясняющий разницу"
    );
}

/// Закрытие по команде: уровня нет, разрыва нет — и это РАЗНЫЕ «нет».
/// `level: None` означает «уровня не было вовсе», `across_gap: Some(false)` —
/// «разрыв проверяли, его не было». Схлопнуть их в одно значило бы потерять
/// различие между «не применимо» и «нет».
#[test]
fn manual_close_distinguishes_absent_level_from_absent_gap() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Long,
        1000,
        6_000_000,
        PD,
        QD,
        None,
        None,
    )
    .unwrap();
    let id = b.positions(ALICE)[0].id;
    b.close(ALICE, id, 6_010_000).unwrap();

    let d = &b.closed_deals(ALICE)[0];
    assert_eq!(d.level, None, "уровня не было вовсе");
    assert_eq!(
        d.across_gap,
        Some(false),
        "а разрыв проверяли — его не было"
    );
}

// ======== Единое определение свободной маржи и стоп-аут (TRM-41) ===========
//
// Числа подобраны так, чтобы порог достигался ТОЧНО, а не «где-то рядом»:
// проверка, которая срабатывает с запасом, не отличает «сработало на пороге» от
// «сработало раньше». Инструмент учебный: pd=2, qd=0 — чтобы арифметику можно
// было проверить в уме, а не доверять ей.

/// `цена × объём` в центах при pd=2, qd=0 — ровно `price_raw * qty`.
const TPD: u8 = 2;
const TQD: u8 = 0;

/// Открыть позицию учебного инструмента.
fn open_t(b: &mut Broker, inst: u32, side: PosSide, entry: i64, qty: i64) -> u64 {
    b.open(
        ALICE,
        &no_marks(),
        inst,
        side,
        qty,
        entry,
        TPD,
        TQD,
        None,
        None,
    )
    .expect("открытие обязано пройти")
}

// ---- Свободная маржа учитывает нереализованный убыток ----------------------

/// **Нереализованный убыток уменьшает свободную маржу.**
///
/// До TRM-41 допуск считал `balance - used_margin` и убытка не замечал: человек
/// с проваленной позицией мог набирать новые на деньги, которых уже нет.
#[test]
fn unrealised_loss_reduces_free_margin() {
    let mut b = Broker::new(20_000, 10);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10); // нотионал $1000, маржа $100

    assert_eq!(b.used_margin(ALICE), 10_000);
    assert_eq!(
        b.free_margin(ALICE, &no_marks()),
        10_000,
        "без движения цены"
    );

    let m = marks(1, 9_000); // -$100
    assert_eq!(b.open_pnl(ALICE, &m), -10_000);
    assert_eq!(b.equity(ALICE, &m), 10_000);
    assert_eq!(
        b.free_margin(ALICE, &m),
        0,
        "убыток съел всё свободное — по балансу его бы ещё хватало"
    );
}

/// И допуск к открытию считает ТЕМ ЖЕ числом. Это главное свойство: экран и
/// движок обязаны отвечать одинаково, иначе отказ приходит на счёт, где «есть
/// деньги».
#[test]
fn admission_uses_the_same_number_as_the_screen() {
    let mut b = Broker::new(20_000, 10);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    let m = marks(1, 9_000);

    assert_eq!(b.free_margin(ALICE, &m), 0, "свободного нет");
    assert_eq!(
        b.open(
            ALICE,
            &m,
            2,
            PosSide::Long,
            10,
            10_000,
            TPD,
            TQD,
            None,
            None
        ),
        Err(BrokerError::InsufficientMargin),
        "и открыть новую нельзя — по балансу было бы можно"
    );
}

// ---- Уровень маржи ---------------------------------------------------------

/// Счёт без позиций: уровень **не определён**. Не ноль и не «бесконечно
/// хорошо» — вопрос ему не задаётся, и делению на ноль взяться неоткуда.
#[test]
fn margin_level_is_undefined_without_positions() {
    // Счёт обязан СУЩЕСТВОВАТЬ, иначе проверяется не то.
    //
    // Первая редакция спрашивала уровень у несуществующего счёта и была зелёной
    // по другой причине: `Broker::margin_level_pct` отдаёт `None` уже потому,
    // что счёта нет, и до расчёта уровня дело не доходит. Мутация «уровень без
    // позиций равен нулю» её не покрасила — тест проверял `accounts.get`, а не
    // уровень. Найдено мутацией, исправлено заведением счёта.
    let mut b = Broker::new(20_000, 10);
    b.open_account(ALICE);
    assert!(
        b.has_account(ALICE),
        "счёт заведён — вопрос есть кому задать"
    );
    assert!(b.positions(ALICE).is_empty(), "и позиций у него нет");

    assert_eq!(
        b.margin_level_pct(ALICE, &no_marks()),
        None,
        "у счёта без позиций уровень НЕ ОПРЕДЕЛЁН: это не ноль"
    );
}

#[test]
fn margin_level_is_equity_over_used_margin() {
    let mut b = Broker::new(20_000, 10);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    assert_eq!(b.margin_level_pct(ALICE, &marks(1, 10_000)), Some(200));
    assert_eq!(b.margin_level_pct(ALICE, &marks(1, 9_000)), Some(100));
    assert_eq!(b.margin_level_pct(ALICE, &marks(1, 8_500)), Some(50));
}

// ---- Принудительное закрытие ----------------------------------------------

/// **Порога нет — закрытия нет.** Счёт проваливается сколь угодно глубоко, и это
/// честное состояние ненастроенного риск-контроля, а не поломка. Выдуманный у
/// нас порог разошёлся бы с тем, по которому считает юрисдикция.
#[test]
fn without_a_threshold_nothing_is_closed() {
    let mut b = Broker::new(20_000, 10);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    let changed = b.check(&marks(1, 1_000), false);
    assert!(changed.is_empty(), "без порога закрывать нечем");
    assert_eq!(b.positions(ALICE).len(), 1, "позиция на месте");
}

/// Порог задан — позиция закрывается, **когда уровень дошёл до порога**, и ни
/// тиком раньше.
#[test]
fn the_position_is_closed_exactly_at_the_threshold() {
    let mut b = Broker::new(20_000, 10).with_stop_out_pct(Some(50));
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);

    // 100% — выше порога, не трогаем.
    assert!(b.check(&marks(1, 9_000), false).is_empty(), "уровень 100%");
    assert_eq!(b.positions(ALICE).len(), 1);

    // 50% — порог достигнут, закрываем.
    let changed = b.check(&marks(1, 8_500), false);
    assert_eq!(changed, vec![ALICE], "счёт обязан быть отмечен изменённым");
    assert!(b.positions(ALICE).is_empty(), "позиция закрыта");
}

/// Закрытие по марже **называется своей причиной**: закрыл риск-контроль, а не
/// человек и не его стоп. На претензию «почему закрыли» это разные ответы.
#[test]
fn a_forced_close_names_its_own_cause() {
    let mut b = Broker::new(20_000, 10).with_stop_out_pct(Some(50));
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    b.check(&marks(1, 8_500), false);

    let ev = b.take_events(ALICE);
    let closed = ev
        .iter()
        .find_map(|e| match e {
            EventKind::PositionClosed { cause, level, .. } => Some((cause, level)),
            _ => None,
        })
        .expect("закрытие обязано быть записано");
    assert_eq!(*closed.0, CloseCause::StopOut);
    assert_eq!(
        *closed.1, None,
        "уровня человек здесь не ставил — приписывать ему наше решение нельзя"
    );
}

/// **Стоп клиента старше принудительного закрытия.** Если на этом же тике
/// сработал его собственный стоп, закрывает он: иначе брокер отбирает
/// исполнение чужого распоряжения и ставит себе причину.
#[test]
fn the_clients_own_stop_wins_on_the_same_tick() {
    let mut b = Broker::new(20_000, 10).with_stop_out_pct(Some(50));
    b.open(
        ALICE,
        &no_marks(),
        1,
        PosSide::Long,
        10,
        10_000,
        TPD,
        TQD,
        Some(8_600), // стоп человека сработает на 8_500
        None,
    )
    .unwrap();

    b.check(&marks(1, 8_500), false);

    let ev = b.take_events(ALICE);
    let cause = ev
        .iter()
        .find_map(|e| match e {
            EventKind::PositionClosed { cause, .. } => Some(*cause),
            _ => None,
        })
        .expect("закрытие");
    assert_eq!(cause, CloseCause::StopLoss, "закрыл стоп человека, а не мы");
}

/// Закрывается **самая убыточная** позиция, и только пока уровень не поднялся.
/// Закрыть всё подряд значило бы отобрать у человека позиции, которые счёт уже
/// не топят.
#[test]
fn the_worst_position_goes_first_and_the_rest_stay() {
    let mut b = Broker::new(20_000, 10).with_stop_out_pct(Some(50));
    open_t(&mut b, 1, PosSide::Long, 5_000, 10); // маржа $50
    open_t(&mut b, 2, PosSide::Long, 5_000, 10); // маржа $50

    // 1 проваливается на -$140, 2 — всего на -$10.
    let m = HashMap::from([(1u32, 3_600i64), (2u32, 4_900i64)]);
    assert_eq!(b.equity(ALICE, &m), 5_000);
    assert_eq!(b.margin_level_pct(ALICE, &m), Some(50), "порог достигнут");

    b.check(&m, false);

    let left = b.positions(ALICE);
    assert_eq!(left.len(), 1, "закрыта одна, а не обе: {left:?}");
    assert_eq!(left[0].instrument, 2, "осталась та, что почти не в убытке");
    assert_eq!(
        b.margin_level_pct(ALICE, &m),
        Some(100),
        "уровень поднялся — цикл обязан был остановиться"
    );
}

/// Позиция **без цены не закрывается**: закрыть по цене входа значило бы
/// выдумать исполнение. Цикл при этом обязан остановиться, а не крутиться.
#[test]
fn a_position_without_a_price_is_not_force_closed() {
    let mut b = Broker::new(20_000, 10).with_stop_out_pct(Some(50));
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    open_t(&mut b, 2, PosSide::Long, 10_000, 10);

    // Цена есть только у первой, и её убыток утягивает уровень ниже порога.
    let m = marks(1, 7_000);
    assert!(b.margin_level_pct(ALICE, &m).unwrap() <= 50);

    b.check(&m, false);

    let left = b.positions(ALICE);
    assert!(
        left.iter().any(|p| p.instrument == 2),
        "позиция без цены обязана остаться: закрывать её нечем по какой цене"
    );
}

// ---- Маржин-колл: предупреждение, а не шум (TRM-50) -----------------------

/// Предупреждение объявляется **один раз на попадание в опасность**.
///
/// Монитор идёт каждые 500 мс. Без защёлки клиент получал бы по два уведомления
/// в секунду всё время, пока счёт в опасности, — а предупреждение, приходящее
/// дважды в секунду, перестают читать. Тогда оно не предупреждение ровно по той
/// же причине, по которой им не является недошедшее.
#[test]
fn a_margin_call_fires_once_per_episode() {
    let mut b = Broker::new(20_000, 10).with_margin_call_pct(Some(100));
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);

    let danger = marks(1, 9_000); // уровень ровно 100%
    assert_eq!(
        b.margin_calls(&danger),
        vec![(ALICE, 100)],
        "первое пересечение"
    );
    assert!(b.margin_calls(&danger).is_empty(), "повтора быть не должно");
    assert!(
        b.margin_calls(&marks(1, 8_900)).is_empty(),
        "и при углублении тоже"
    );
}

/// Выбрался — защёлка снимается, и следующее попадание снова предупредят.
#[test]
fn recovering_re_arms_the_warning() {
    let mut b = Broker::new(20_000, 10).with_margin_call_pct(Some(100));
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);

    assert_eq!(b.margin_calls(&marks(1, 9_000)).len(), 1);
    assert!(
        b.margin_calls(&marks(1, 10_000)).is_empty(),
        "вышел наверх — не предупреждаем"
    );
    assert_eq!(
        b.margin_calls(&marks(1, 9_000)).len(),
        1,
        "вернулся в опасность — предупреждаем снова"
    );
}

/// Счёт без позиций опасности не представляет, и защёлка обязана сняться:
/// иначе закрывший всё и открывший заново не будет предупреждён.
#[test]
fn closing_everything_re_arms_the_warning() {
    let mut b = Broker::new(20_000, 10).with_margin_call_pct(Some(100));
    let id = open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    assert_eq!(b.margin_calls(&marks(1, 9_000)).len(), 1);

    b.close(ALICE, id, 9_000).expect("закрытие");
    assert!(
        b.margin_calls(&marks(1, 9_000)).is_empty(),
        "позиций нет — опасности нет"
    );

    open_t(&mut b, 1, PosSide::Long, 9_000, 10);
    assert_eq!(
        b.margin_calls(&marks(1, 8_100)).len(),
        1,
        "новая опасность — новое предупреждение"
    );
}

/// **Порога нет — предупреждения нет.** Как и у закрытия: уровень задаёт
/// юрисдикция, выдумывать его здесь нельзя.
#[test]
fn without_a_call_level_nothing_is_warned() {
    let mut b = Broker::new(20_000, 10);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    assert!(b.margin_calls(&marks(1, 1_000)).is_empty());
}

/// **Предупреждение видно даже тогда, когда закрытие случилось тем же тиком.**
///
/// Это и есть «закрытие без предупреждения — срабатывание без шанса»: если
/// спросить о пересечении ПОСЛЕ прогона триггеров, уровень уже поднят закрытием,
/// и предупреждения не будет никогда.
#[test]
fn the_warning_is_visible_even_when_the_stop_out_happens_on_the_same_tick() {
    let mut b = Broker::new(20_000, 10)
        .with_margin_call_pct(Some(100))
        .with_stop_out_pct(Some(50));
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);

    // Цена сразу ниже уровня закрытия: на этом тике произойдёт и то, и другое.
    let crash = marks(1, 8_500);

    let called = b.margin_calls(&crash);
    assert_eq!(
        called,
        vec![(ALICE, 50)],
        "предупреждение обязано быть видно"
    );

    b.check(&crash, false);
    assert!(b.positions(ALICE).is_empty(), "и закрытие тоже произошло");

    // Порядок проверяется тем, что обратный порядок даёт пустоту: после
    // закрытия позиций нет, уровень не определён, пересечения не видно.
    assert!(
        b.margin_calls(&crash).is_empty(),
        "после закрытия предупреждать уже не о чем — потому оно и идёт первым"
    );
}
