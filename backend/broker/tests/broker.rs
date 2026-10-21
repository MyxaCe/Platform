//! Тесты бумажного брокера: маржа, P&L, закрытие, лимитные ордера, SL/TP, история.

use std::collections::HashMap;

use broker::journal::{CloseCause, EventKind};
use broker::{Broker, BrokerError, PosSide};
use domain::account::UserId;

const ALICE: UserId = UserId(1);
const PD: u8 = 2;
const QD: u8 = 5;
// BTC pd=2 qd=5: цена 60000.00 → raw 6_000_000; объём 0.01 → raw 1000; нотионал = $600 = 60000 центов.

fn marks(inst: u32, price: i64) -> HashMap<u32, i64> {
    HashMap::from([(inst, price)])
}
fn open(b: &mut Broker, side: PosSide, price: i64) -> u64 {
    b.open(ALICE, 1, side, 1000, price, PD, QD, None, None).unwrap()
}

// ---- Базовое ---------------------------------------------------------------

#[test]
fn open_locks_margin() {
    let mut b = Broker::new(1_000_000, 1);
    open(&mut b, PosSide::Long, 6_000_000);
    assert_eq!(b.used_margin(ALICE), 60_000);
    assert_eq!(b.free_margin(ALICE), 940_000);
    assert_eq!(b.balance(ALICE), 1_000_000);
}

#[test]
fn insufficient_margin_is_rejected() {
    let mut b = Broker::new(50_000, 1);
    assert_eq!(b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, None, None), Err(BrokerError::InsufficientMargin));
}

#[test]
fn long_pnl_and_short_pnl() {
    let mut b = Broker::new(1_000_000, 1);
    open(&mut b, PosSide::Long, 6_000_000);
    assert_eq!(b.open_pnl(ALICE, &marks(1, 6_100_000)), 1000); // +$10
    let mut b2 = Broker::new(1_000_000, 1);
    b2.open(ALICE, 1, PosSide::Short, 1000, 6_000_000, PD, QD, None, None).unwrap();
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
    assert_eq!(b.cancel_pending(ALICE, 999), Err(BrokerError::UnknownPending));
}

// ---- SL/TP -----------------------------------------------------------------

#[test]
fn stop_loss_closes_position() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, Some(5_950_000), Some(6_100_000)).unwrap();
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
    b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, Some(5_950_000), Some(6_100_000)).unwrap();
    b.check(&marks(1, 6_100_000), false); // достигли TP
    assert!(b.positions(ALICE).is_empty());
    assert_eq!(b.closed_deals(ALICE)[0].pnl, 1000);
}

// ---- Персистентность (ADR-016) --------------------------------------------

#[test]
fn check_reports_only_changed_accounts() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, Some(5_950_000), None).unwrap();

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
    b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, Some(5_950_000), None).unwrap();
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
    assert_eq!(b.withdraw(ALICE, 400_000).unwrap(), 600_000);
    assert_eq!(b.balance(ALICE), 600_000);
}

#[test]
fn cannot_withdraw_more_than_free() {
    let mut b = Broker::new(1_000_000, 1);
    assert_eq!(b.withdraw(ALICE, 1_000_001), Err(BrokerError::InsufficientFunds));
    assert_eq!(b.balance(ALICE), 1_000_000, "неудачный вывод не меняет баланс");
}

#[test]
fn margin_locks_funds_from_withdrawal() {
    let mut b = Broker::new(1_000_000, 1);
    // Открываем позицию: нотионал = маржа при leverage=1 → часть баланса занята.
    b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, None, None).unwrap();
    let free = b.free_margin(ALICE);
    assert!(free < 1_000_000, "часть средств ушла под маржу");
    // Больше свободного вывести нельзя.
    assert_eq!(b.withdraw(ALICE, free + 1), Err(BrokerError::InsufficientFunds));
    // Ровно свободное — можно.
    assert!(b.withdraw(ALICE, free).is_ok());
}
// ---- Разрыв ряда, проскальзывание и правда в журнале (TRM-19, TRM-31) -----

/// Рынок на разрыве перепрыгнул стоп. Исполняем по НАБЛЮДЁННОЙ цене (Р-178):
/// гарантия уровня означала бы, что разницу платит брокер — на каждом скачке и
/// без предела. В журнал идут обе цены, иначе проскальзывание потом не из чего
/// восстановить.
#[test]
fn gap_close_executes_at_the_observed_price_and_records_the_level() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, Some(5_950_000), None).unwrap();

    assert_eq!(b.check(&marks(1, 5_400_000), true), vec![ALICE], "стоп обязан сработать и на разрыве");

    let closed = b.closed_deals(ALICE);
    assert_eq!(closed[0].exit, 5_400_000, "исполнено по рынку, а не по уровню");

    match b.take_events(ALICE).last().expect("закрытие обязано быть в событиях") {
        EventKind::PositionClosed { exit, level, across_gap, cause, .. } => {
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
        b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, Some(5_950_000), None).unwrap();
        b.check(&marks(1, mark), true);
        b.closed_deals(ALICE)[0].pnl
    };
    let small = loss_at(5_940_000);
    let big = loss_at(5_400_000);
    assert!(big < small, "чем дальше рынок перепрыгнул стоп, тем больше потерял клиент: {big} против {small}");
}

/// Когда рынок дошёл ровно до уровня, проскальзывания нет и разрыв не
/// объявляется. Иначе признак стоял бы всегда и его перестали бы читать.
#[test]
fn quiet_close_has_no_slippage_and_is_not_marked_as_a_gap() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, Some(5_950_000), None).unwrap();
    b.check(&marks(1, 5_950_000), false);

    match b.take_events(ALICE).last().expect("закрытие") {
        EventKind::PositionClosed { exit, level, across_gap, .. } => {
            assert_eq!(*level, Some(*exit), "исполнилось ровно по поставленному — проскальзывания нет");
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
        b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, None, Some(6_100_000)).unwrap();
        b.check(&marks(1, mark), true);
        let closed = b.closed_deals(ALICE);
        (closed[0].exit, closed[0].pnl)
    };

    let (exit_at_level, pnl_at_level) = pnl_at(6_100_000);
    let (exit_gapped, pnl_gapped) = pnl_at(6_300_000);

    assert_eq!(exit_at_level, 6_100_000, "рынок дошёл ровно до тейка — исполнение совпало с уровнем");
    assert_eq!(exit_gapped, 6_300_000, "рынок перепрыгнул тейк — исполнено по рынку, а не по уровню");
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
    b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, None, None).unwrap();
    let id = b.positions(ALICE)[0].id;
    b.close(ALICE, id, 6_010_000).unwrap();

    match b.take_events(ALICE).last().expect("закрытие") {
        EventKind::PositionClosed { level, across_gap, cause, .. } => {
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
    assert_eq!(pos[0].entry, 5_300_000, "вход по рынку: клиент купил дешевле, чем просил");

    let ev = b.take_events(ALICE);
    let fill = ev
        .iter()
        .find(|k| matches!(k, EventKind::PendingTriggered { .. }))
        .expect("сработавший ордер обязан быть в событиях");
    match fill {
        EventKind::PendingTriggered { price, exec, across_gap, .. } => {
            assert_eq!(*price, 5_900_000, "уровень ордера");
            assert_eq!(*exec, Some(5_300_000), "и цена, по которой он исполнен");
            assert!(*across_gap);
        }
        other => panic!("ожидался сработавший ордер, а не {other:?}"),
    }
}
