use std::collections::HashMap;

use broker::journal::{CloseCause, EventKind};
use broker::{Broker, BrokerError, PosSide};
use domain::account::UserId;

fn no_marks() -> std::collections::HashMap<u32, i64> {
    std::collections::HashMap::new()
}

const ALICE: UserId = UserId(1);
const PD: u8 = 2;
const QD: u8 = 5;

fn marks(inst: u32, price: i64) -> HashMap<u32, i64> {
    HashMap::from([(inst, price)])
}
fn open(b: &mut Broker, side: PosSide, price: i64) -> u64 {
    b.open(ALICE, &no_marks(), 1, side, 1000, price, PD, QD, None, None)
        .unwrap()
}

#[test]
fn open_locks_margin() {
    let mut b = Broker::new(1_000_000, 1);
    open(&mut b, PosSide::Long, 6_000_000);
    assert_eq!(b.used_margin(ALICE), 60_000);
    assert_eq!(
        b.free_margin(ALICE, &no_marks()).expect("счёт заведён"),
        940_000
    );
    assert_eq!(b.balance(ALICE).expect("счёт заведён"), 1_000_000);
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
    assert_eq!(b.open_pnl(ALICE, &marks(1, 6_100_000)), 1000);
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
    assert_eq!(b2.open_pnl(ALICE, &marks(1, 5_900_000)), 1000);
}

#[test]
fn close_realizes_pnl_and_records_history() {
    let mut b = Broker::new(1_000_000, 1);
    let id = open(&mut b, PosSide::Long, 6_000_000);
    let pnl = b.close(ALICE, id, 6_100_000).unwrap();
    assert_eq!(pnl, 1000);
    assert_eq!(b.balance(ALICE).expect("счёт заведён"), 1_001_000);
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
    assert_eq!(b.balance(ALICE).expect("счёт заведён"), 999_000);
}

#[test]
fn pending_buy_triggers_when_price_drops() {
    let mut b = Broker::new(1_000_000, 1);
    let pid = b
        .place_pending(ALICE, 1, PosSide::Long, 1000, 5_900_000, PD, QD, None, None)
        .expect("ордер поставлен");
    assert_eq!(b.pendings(ALICE).len(), 1);
    b.check(&marks(1, 6_000_000), false);
    assert_eq!(b.pendings(ALICE).len(), 1);
    b.check(&marks(1, 5_900_000), false);
    assert!(b.pendings(ALICE).is_empty());
    assert_eq!(b.positions(ALICE).len(), 1);
    let _ = pid;
}

#[test]
fn cancel_pending() {
    let mut b = Broker::new(1_000_000, 1);
    let pid = b
        .place_pending(ALICE, 1, PosSide::Long, 1000, 5_900_000, PD, QD, None, None)
        .expect("ордер поставлен");
    assert_eq!(b.cancel_pending(ALICE, pid), Ok(()));
    assert!(b.pendings(ALICE).is_empty());
    assert_eq!(
        b.cancel_pending(ALICE, 999),
        Err(BrokerError::UnknownPending)
    );
}

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
    b.check(&marks(1, 5_990_000), false);
    assert_eq!(b.positions(ALICE).len(), 1);
    b.check(&marks(1, 5_950_000), false);
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
    b.check(&marks(1, 6_100_000), false);
    assert!(b.positions(ALICE).is_empty());
    assert_eq!(b.closed_deals(ALICE)[0].pnl, 1000);
}

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

    assert!(b.check(&marks(1, 5_990_000), false).is_empty());

    assert_eq!(b.check(&marks(1, 5_950_000), false), vec![ALICE]);

    assert!(b.check(&marks(1, 5_900_000), false).is_empty());
}

#[test]
fn check_reports_account_on_pending_fill() {
    let mut b = Broker::new(1_000_000, 1);
    b.place_pending(ALICE, 1, PosSide::Long, 1000, 5_900_000, PD, QD, None, None)
        .expect("ордер поставлен");
    assert!(b.check(&marks(1, 6_000_000), false).is_empty());
    assert_eq!(b.check(&marks(1, 5_900_000), false), vec![ALICE]);
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
    b.place_pending(ALICE, 2, PosSide::Short, 500, 7_000_000, PD, QD, None, None)
        .expect("ордер поставлен");
    b.check(&marks(1, 5_950_000), false);
    let snap = b.snapshot(ALICE);
    assert_eq!(snap.pendings.len(), 1);
    assert_eq!(snap.closed.len(), 1);

    let mut fresh = Broker::new(1_000_000, 1);
    fresh.restore(ALICE, snap);
    assert_eq!(
        fresh.balance(ALICE).expect("счёт заведён"),
        b.balance(ALICE).expect("счёт заведён")
    );
    assert_eq!(fresh.pendings(ALICE).len(), 1);
    assert_eq!(fresh.closed_deals(ALICE).len(), 1);
    assert_eq!(fresh.users(), vec![ALICE]);

    let used: Vec<u64> = fresh.pendings(ALICE).iter().map(|p| p.id).collect();
    let new_id = fresh
        .place_pending(ALICE, 3, PosSide::Long, 100, 1_000_000, PD, QD, None, None)
        .expect("ордер поставлен");
    assert!(!used.contains(&new_id));
}

#[test]
fn snapshot_of_unknown_user_is_start_balance() {
    let b = Broker::new(1_000_000, 1);
    let snap = b.snapshot(ALICE);
    assert_eq!(snap.balance, 1_000_000);
    assert!(snap.positions.is_empty() && snap.closed.is_empty());
}

#[test]
fn deposit_increases_balance() {
    let mut b = Broker::new(1_000_000, 1);
    assert_eq!(
        b.balance(ALICE),
        None,
        "счёта ещё нет — остатка нет, а не стартовая сумма площадки"
    );
    b.open_account(ALICE);
    assert_eq!(b.balance(ALICE), Some(1_000_000));
    assert_eq!(b.deposit(ALICE, 500_000), 1_500_000);
    assert_eq!(b.balance(ALICE), Some(1_500_000));
}

#[test]
fn withdraw_decreases_balance() {
    let mut b = Broker::new(1_000_000, 1);
    b.open_account(ALICE);
    assert_eq!(b.withdraw(ALICE, 400_000, &no_marks()).unwrap(), 600_000);
    assert_eq!(b.balance(ALICE).expect("счёт заведён"), 600_000);
}

#[test]
fn cannot_withdraw_from_an_account_that_does_not_exist() {
    let mut b = Broker::new(1_000_000, 1);
    assert_eq!(
        b.withdraw(ALICE, 1, &no_marks()),
        Err(BrokerError::InsufficientFunds),
        "счёта нет — выводить нечего"
    );
    assert_eq!(b.balance(ALICE), None, "неудачная попытка не заводит счёт");
}

#[test]
fn cannot_withdraw_more_than_free() {
    let mut b = Broker::new(1_000_000, 1);
    b.open_account(ALICE);
    assert_eq!(
        b.withdraw(ALICE, 1_000_001, &no_marks()),
        Err(BrokerError::InsufficientFunds)
    );
    assert_eq!(
        b.balance(ALICE),
        Some(1_000_000),
        "неудачный вывод не меняет баланс"
    );
}

#[test]
fn margin_locks_funds_from_withdrawal() {
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
    let free = b.free_margin(ALICE, &no_marks()).expect("счёт заведён");
    assert!(free < 1_000_000, "часть средств ушла под маржу");
    assert_eq!(
        b.withdraw(ALICE, free + 1, &no_marks()),
        Err(BrokerError::InsufficientFunds)
    );
    assert!(b.withdraw(ALICE, free, &no_marks()).is_ok());
}

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

#[test]
fn take_profit_slips_in_the_clients_favour() {
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

#[test]
fn pending_fill_on_a_gap_opens_at_the_market_not_at_the_level() {
    let mut b = Broker::new(1_000_000, 1);
    b.place_pending(ALICE, 1, PosSide::Long, 100, 5_900_000, PD, QD, None, None)
        .expect("ордер поставлен");
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

const TPD: u8 = 2;
const TQD: u8 = 0;

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

#[test]
fn unrealised_loss_reduces_free_margin() {
    let mut b = Broker::new(20_000, 10);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);

    assert_eq!(b.used_margin(ALICE), 10_000);
    assert_eq!(
        b.free_margin(ALICE, &no_marks()).expect("счёт заведён"),
        10_000,
        "без движения цены"
    );

    let m = marks(1, 9_000);
    assert_eq!(b.open_pnl(ALICE, &m), -10_000);
    assert_eq!(b.equity(ALICE, &m).expect("счёт заведён"), 10_000);
    assert_eq!(
        b.free_margin(ALICE, &m).expect("счёт заведён"),
        0,
        "убыток съел всё свободное — по балансу его бы ещё хватало"
    );
}

#[test]
fn admission_uses_the_same_number_as_the_screen() {
    let mut b = Broker::new(20_000, 10);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    let m = marks(1, 9_000);

    assert_eq!(
        b.free_margin(ALICE, &m).expect("счёт заведён"),
        0,
        "свободного нет"
    );
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

#[test]
fn margin_level_is_undefined_without_positions() {
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

#[test]
fn without_a_threshold_nothing_is_closed() {
    let mut b = Broker::new(20_000, 10);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    let changed = b.check(&marks(1, 1_000), false);
    assert!(changed.is_empty(), "без порога закрывать нечем");
    assert_eq!(b.positions(ALICE).len(), 1, "позиция на месте");
}

#[test]
fn the_position_is_closed_exactly_at_the_threshold() {
    let mut b = Broker::new(20_000, 10).with_stop_out_pct(Some(50));
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);

    assert!(b.check(&marks(1, 9_000), false).is_empty(), "уровень 100%");
    assert_eq!(b.positions(ALICE).len(), 1);

    let changed = b.check(&marks(1, 8_500), false);
    assert_eq!(changed, vec![ALICE], "счёт обязан быть отмечен изменённым");
    assert!(b.positions(ALICE).is_empty(), "позиция закрыта");
}

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
        Some(8_600),
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

#[test]
fn the_worst_position_goes_first_and_the_rest_stay() {
    let mut b = Broker::new(20_000, 10).with_stop_out_pct(Some(50));
    open_t(&mut b, 1, PosSide::Long, 5_000, 10);
    open_t(&mut b, 2, PosSide::Long, 5_000, 10);

    let m = HashMap::from([(1u32, 3_600i64), (2u32, 4_900i64)]);
    assert_eq!(b.equity(ALICE, &m).expect("счёт заведён"), 5_000);
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

#[test]
fn a_position_without_a_price_is_not_force_closed() {
    let mut b = Broker::new(20_000, 10).with_stop_out_pct(Some(50));
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    open_t(&mut b, 2, PosSide::Long, 10_000, 10);

    let m = marks(1, 7_000);
    assert!(b.margin_level_pct(ALICE, &m).unwrap() <= 50);

    b.check(&m, false);

    let left = b.positions(ALICE);
    assert!(
        left.iter().any(|p| p.instrument == 2),
        "позиция без цены обязана остаться: закрывать её нечем по какой цене"
    );
}

#[test]
fn a_margin_call_fires_once_per_episode() {
    let mut b = Broker::new(20_000, 10).with_margin_call_pct(Some(100));
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);

    let danger = marks(1, 9_000);
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

#[test]
fn the_rearm_band_absorbs_jitter() {
    let mut b = Broker::new(20_000, 10)
        .with_margin_call_pct(Some(100))
        .with_margin_call_rearm_band_pct(10);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);

    assert_eq!(
        b.margin_calls(&marks(1, 9_000)).len(),
        1,
        "уровень 100% — предупредили"
    );
    assert!(
        b.margin_calls(&marks(1, 9_050)).is_empty(),
        "105% ещё в полосе"
    );
    assert!(
        b.margin_calls(&marks(1, 9_000)).is_empty(),
        "и обратно вниз — без повтора"
    );
    assert!(
        b.margin_calls(&marks(1, 9_110)).is_empty(),
        "выход не предупреждение"
    );
    assert_eq!(
        b.margin_calls(&marks(1, 9_000)).len(),
        1,
        "вышел за полосу и вернулся — предупреждаем снова"
    );
}

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

#[test]
fn without_a_call_level_nothing_is_warned() {
    let mut b = Broker::new(20_000, 10);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    assert!(b.margin_calls(&marks(1, 1_000)).is_empty());
}

#[test]
fn the_warning_is_visible_even_when_the_stop_out_happens_on_the_same_tick() {
    let mut b = Broker::new(20_000, 10)
        .with_margin_call_pct(Some(100))
        .with_stop_out_pct(Some(50));
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);

    let crash = marks(1, 8_500);

    let called = b.margin_calls(&crash);
    assert_eq!(
        called,
        vec![(ALICE, 50)],
        "предупреждение обязано быть видно"
    );

    b.check(&crash, false);
    assert!(b.positions(ALICE).is_empty(), "и закрытие тоже произошло");

    assert!(
        b.margin_calls(&crash).is_empty(),
        "после закрытия предупреждать уже не о чем — потому оно и идёт первым"
    );
}

fn blown_account(protect: bool) -> Broker {
    let mut b = Broker::new(20_000, 10)
        .with_stop_out_pct(Some(50))
        .with_negative_balance_protection(protect);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    b
}

#[test]
fn a_blown_account_is_brought_back_to_zero_by_a_named_posting() {
    let mut b = blown_account(true);
    b.check(&marks(1, 5_000), false);

    assert!(b.positions(ALICE).is_empty(), "позицию закрыли");
    assert_eq!(
        b.balance(ALICE).expect("счёт заведён"),
        0,
        "остаток обязан вернуться к нулю"
    );

    let ev = b.take_events(ALICE);
    let amount = ev
        .iter()
        .find_map(|e| match e {
            EventKind::NegativeBalanceCompensated { amount } => Some(*amount),
            _ => None,
        })
        .expect("компенсация обязана быть записана отдельной статьёй");
    assert!(
        amount > 0,
        "сумма компенсации — это наши деньги, и она обязана быть в записи, а не выводиться из нуля"
    );
}

#[test]
fn without_the_flag_the_balance_stays_negative() {
    let mut b = blown_account(false);
    b.check(&marks(1, 5_000), false);

    assert!(b.positions(ALICE).is_empty());
    assert!(
        b.balance(ALICE).expect("счёт заведён") < 0,
        "защиты нет — долг остаётся"
    );
    let ev = b.take_events(ALICE);
    assert!(
        !ev.iter()
            .any(|e| matches!(e, EventKind::NegativeBalanceCompensated { .. })),
        "и проводки компенсации быть не должно"
    );
}

#[test]
fn an_open_position_is_not_compensated_early() {
    let mut b = Broker::new(20_000, 10)
        .with_negative_balance_protection(true)
        .with_stop_out_pct(None);
    open_t(&mut b, 1, PosSide::Long, 10_000, 10);
    let id2 = open_t(&mut b, 2, PosSide::Long, 10_000, 10);
    b.close(ALICE, id2, 7_000).expect("закрытие второй");

    assert!(
        b.balance(ALICE).expect("счёт заведён") < 0,
        "остаток ушёл в минус"
    );
    assert!(!b.positions(ALICE).is_empty(), "но позиция ещё открыта");

    b.check(&marks(1, 10_000), false);
    assert!(
        b.balance(ALICE).expect("счёт заведён") < 0,
        "пока есть из чего выбраться, не гасим"
    );
}

#[test]
fn the_compensation_replays_from_its_own_amount() {
    let mut b = blown_account(true);
    b.check(&marks(1, 5_000), false);
    let kinds = b.take_events(ALICE);

    let mut j = broker::journal::Journal::new(20_000);
    for e in j.seal(1, &broker::journal::Origin::System, &kinds) {
        j.commit(e).expect("лента");
    }
    assert_eq!(
        j.replay().balance,
        b.balance(ALICE).expect("счёт заведён"),
        "свёртка обязана сойтись со счётом"
    );
    assert_eq!(j.replay().balance, 0, "и это ноль, выведенный из проводок");
}

use broker::start_balance_for;
use domain::account::{
    AccountKind, AccountOrigin, AccountProfile, CategoryAtOpen, ClientCategory, Currency,
};

fn real_profile(owner: UserId) -> AccountProfile {
    AccountProfile {
        owner,
        kind: AccountKind::Real,
        origin: AccountOrigin::Registration,
        category_at_open: CategoryAtOpen(ClientCategory::Retail),
        currency: Currency::USD,
    }
}

#[test]
fn a_real_account_opens_with_nothing() {
    assert_eq!(start_balance_for(AccountKind::Real, 1_000_000), 0);
    assert_eq!(
        start_balance_for(AccountKind::Demo, 1_000_000),
        1_000_000,
        "а демо — настроенной суммой"
    );

    let mut b = Broker::new(1_000_000, 1);
    assert!(b.open_account_with(ALICE, real_profile(ALICE), b.start_balance()));
    assert_eq!(
        b.balance(ALICE).expect("счёт заведён"),
        0,
        "правило применяется при заведении, а не на словах"
    );
}

#[test]
fn the_profile_travels_in_the_opening_record() {
    let mut b = Broker::new(1_000_000, 1);
    b.open_account_with(ALICE, real_profile(ALICE), b.start_balance());
    let ev = b.take_events(ALICE);
    match ev.first().expect("запись о заведении") {
        EventKind::AccountOpened { balance, profile } => {
            assert_eq!(*balance, 0);
            assert_eq!(profile.kind, AccountKind::Real);
            assert_eq!(profile.owner, ALICE);
        }
        other => panic!("не та запись: {other:?}"),
    }
}

#[test]
fn the_profile_survives_a_snapshot_round_trip() {
    let mut b = Broker::new(1_000_000, 1);
    b.open_account_with(ALICE, real_profile(ALICE), b.start_balance());
    let snap = b.snapshot(ALICE);
    assert_eq!(snap.profile.kind, AccountKind::Real);

    let mut other = Broker::new(1_000_000, 1);
    other.restore(ALICE, snap);
    assert_eq!(other.snapshot(ALICE).profile.kind, AccountKind::Real);
}

#[test]
fn trading_does_not_change_the_profile() {
    let mut b = Broker::new(1_000_000, 1);
    b.open_account_with(ALICE, AccountProfile::legacy_demo(ALICE), b.start_balance());
    let before = b.snapshot(ALICE).profile;
    open(&mut b, PosSide::Long, 6_000_000);
    assert_eq!(
        b.snapshot(ALICE).profile,
        before,
        "сделка профиль не трогает"
    );
}

#[test]
fn opening_an_existing_account_is_refused() {
    let mut b = Broker::new(1_000_000, 1);
    assert!(b.open_account_with(ALICE, AccountProfile::legacy_demo(ALICE), b.start_balance()));
    assert!(
        !b.open_account_with(ALICE, real_profile(ALICE), b.start_balance()),
        "счёт уже есть — подменять его род нельзя"
    );
    assert_eq!(b.snapshot(ALICE).profile.kind, AccountKind::Demo);
}

fn account_in(code: &str) -> (Broker, AccountProfile) {
    let cur = Currency::parse(code).unwrap_or_else(|| panic!("{code} обязана быть известна"));
    let profile = AccountProfile {
        owner: ALICE,
        currency: cur,
        ..Default::default()
    };
    (Broker::new(100_000_000, 1), profile)
}

#[test]
fn margin_scales_with_the_currency_not_with_a_hardwired_cent() {
    let expected = [("JPY", 100i128), ("USD", 10_000), ("BHD", 100_000)];
    for (code, want) in expected {
        let (mut b, profile) = account_in(code);
        b.open_account_with(ALICE, profile, b.start_balance());
        b.open(
            ALICE,
            &no_marks(),
            1,
            PosSide::Long,
            1,
            10_000,
            2,
            0,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("{code}: открытие {e:?}"));
        assert_eq!(
            b.used_margin(ALICE),
            want,
            "{code}: занятая маржа обязана быть в наименьших единицах ЭТОЙ валюты"
        );
    }
}

#[test]
fn unrealised_pnl_scales_with_the_currency() {
    let expected = [("JPY", 1i128), ("USD", 100), ("BHD", 1_000)];
    for (code, want) in expected {
        let (mut b, profile) = account_in(code);
        b.open_account_with(ALICE, profile, b.start_balance());
        b.open(
            ALICE,
            &no_marks(),
            1,
            PosSide::Long,
            1,
            10_000,
            2,
            0,
            None,
            None,
        )
        .unwrap();
        assert_eq!(b.open_pnl(ALICE, &marks(1, 10_100)), want, "{code}");
    }
}

#[test]
fn the_margin_invariant_speaks_the_accounts_currency() {
    for code in ["JPY", "USD", "BHD"] {
        let (mut b, profile) = account_in(code);
        b.open_account_with(ALICE, profile, b.start_balance());
        b.open(
            ALICE,
            &no_marks(),
            1,
            PosSide::Long,
            1,
            10_000,
            2,
            0,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            broker::check_snapshot(&b.snapshot(ALICE)),
            vec![],
            "{code}: счёт здоров, и проверка обязана это видеть"
        );
    }
}

#[test]
fn a_reset_is_recorded_as_a_signed_movement() {
    let mut b = Broker::new(1_000_000, 1);
    b.open_account_with(ALICE, AccountProfile::legacy_demo(ALICE), b.start_balance());
    let _ = b.take_events(ALICE);

    let id = open(&mut b, PosSide::Long, 6_000_000);
    b.close(ALICE, id, 5_000_000).expect("закрытие");
    let lost = 1_000_000 - b.balance(ALICE).expect("счёт заведён");
    assert!(lost > 0, "счёт обязан просесть, иначе проверять нечего");
    let _ = b.take_events(ALICE);

    assert_eq!(b.reset_demo(ALICE).expect("сброс"), 1_000_000);
    match b.take_events(ALICE).as_slice() {
        [EventKind::DemoAccountReset { adjustment }] => {
            assert_eq!(
                *adjustment, lost,
                "в записи — ровно столько, сколько добавили"
            );
        }
        other => panic!("ожидалась одна проводка сброса: {other:?}"),
    }
}

#[test]
fn a_reset_can_move_the_balance_down() {
    let mut b = Broker::new(1_000_000, 1);
    b.open_account_with(ALICE, AccountProfile::legacy_demo(ALICE), b.start_balance());
    b.deposit(ALICE, 500_000);
    let _ = b.take_events(ALICE);

    assert_eq!(b.reset_demo(ALICE).expect("сброс"), 1_000_000);
    match b.take_events(ALICE).as_slice() {
        [EventKind::DemoAccountReset { adjustment }] => assert_eq!(*adjustment, -500_000),
        other => panic!("{other:?}"),
    }
}

#[test]
fn resetting_an_untouched_account_writes_nothing() {
    let mut b = Broker::new(1_000_000, 1);
    b.open_account_with(ALICE, AccountProfile::legacy_demo(ALICE), b.start_balance());
    let _ = b.take_events(ALICE);
    assert_eq!(b.reset_demo(ALICE).expect("сброс"), 1_000_000);
    assert!(b.take_events(ALICE).is_empty());
}

#[test]
fn a_reset_is_refused_while_the_account_is_not_flat() {
    let mut b = Broker::new(1_000_000, 1);
    b.open_account_with(ALICE, AccountProfile::legacy_demo(ALICE), b.start_balance());
    open(&mut b, PosSide::Long, 6_000_000);
    assert_eq!(b.reset_demo(ALICE), Err(BrokerError::AccountNotFlat));

    let mut b2 = Broker::new(1_000_000, 1);
    b2.open_account_with(
        ALICE,
        AccountProfile::legacy_demo(ALICE),
        b2.start_balance(),
    );
    b2.place_pending(ALICE, 1, PosSide::Long, 1000, 5_000_000, PD, QD, None, None)
        .expect("ордер поставлен");
    assert_eq!(b2.reset_demo(ALICE), Err(BrokerError::AccountNotFlat));
}

#[test]
fn a_real_account_cannot_be_reset() {
    let mut b = Broker::new(1_000_000, 1);
    b.open_account_with(ALICE, real_profile(ALICE), b.start_balance());
    assert_eq!(b.reset_demo(ALICE), Err(BrokerError::NotADemoAccount));
}

#[test]
fn resetting_a_missing_account_says_so() {
    let mut b = Broker::new(1_000_000, 1);
    assert_eq!(b.reset_demo(ALICE), Err(BrokerError::UnknownAccount));
}

#[test]
fn a_level_already_past_the_price_is_refused_not_opened() {
    const MARK: i64 = 8_054_279;
    let cases = [
        (
            PosSide::Long,
            Some(8_100_000),
            None,
            BrokerError::StopLossAlreadyReached,
        ),
        (
            PosSide::Long,
            None,
            Some(82_000),
            BrokerError::TakeProfitAlreadyReached,
        ),
        (
            PosSide::Short,
            Some(8_000_000),
            None,
            BrokerError::StopLossAlreadyReached,
        ),
        (
            PosSide::Short,
            None,
            Some(8_100_000),
            BrokerError::TakeProfitAlreadyReached,
        ),
    ];
    for (side, sl, tp, want) in cases {
        let mut b = Broker::new(100_000_000, 1);
        let got = b.open(ALICE, &marks(1, MARK), 1, side, 1000, MARK, PD, QD, sl, tp);
        assert_eq!(got, Err(want), "{side:?} sl={sl:?} tp={tp:?}");
        assert!(
            b.positions(ALICE).is_empty(),
            "отказ не оставляет позиции: {side:?} sl={sl:?} tp={tp:?}"
        );
    }
}

#[test]
fn levels_on_the_right_side_of_the_price_are_accepted() {
    const MARK: i64 = 8_054_279;
    let mut b = Broker::new(100_000_000, 1);
    b.open(
        ALICE,
        &marks(1, MARK),
        1,
        PosSide::Long,
        1000,
        MARK,
        PD,
        QD,
        Some(8_000_000),
        Some(8_100_000),
    )
    .expect("стоп ниже, тейк выше — обычная покупка");
    assert_eq!(b.positions(ALICE).len(), 1);
}

#[test]
fn pending_levels_are_checked_against_its_own_price() {
    const MARKET: i64 = 8_054_279;
    const LIMIT: i64 = 7_500_000;
    const _: () = assert!(MARKET > LIMIT);

    let mut b = Broker::new(100_000_000, 1);
    assert_eq!(
        b.place_pending(
            ALICE,
            1,
            PosSide::Long,
            1000,
            LIMIT,
            PD,
            QD,
            Some(7_600_000),
            None
        ),
        Err(BrokerError::StopLossAlreadyReached),
        "стоп выше цены ордера — позиция родилась бы уже закрытой"
    );
    assert!(b.pendings(ALICE).is_empty(), "отказ не оставляет ордера");

    b.place_pending(
        ALICE,
        1,
        PosSide::Long,
        1000,
        LIMIT,
        PD,
        QD,
        Some(7_400_000),
        None,
    )
    .expect("стоп ниже цены ордера — обычный лимитный ордер");
    assert_eq!(b.pendings(ALICE).len(), 1);
}
