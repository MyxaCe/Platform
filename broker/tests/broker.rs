//! Тесты бумажного брокера: маржа, P&L, закрытие, лимитные ордера, SL/TP, история.

use std::collections::HashMap;

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
    b.check(&marks(1, 6_000_000)); // цена выше лимита — не сработал
    assert_eq!(b.pendings(ALICE).len(), 1);
    b.check(&marks(1, 5_900_000)); // цена дошла до лимита — сработал
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
    b.check(&marks(1, 5_990_000)); // между SL и TP — не трогаем
    assert_eq!(b.positions(ALICE).len(), 1);
    b.check(&marks(1, 5_950_000)); // достигли SL — закрыли
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
    b.check(&marks(1, 6_100_000)); // достигли TP
    assert!(b.positions(ALICE).is_empty());
    assert_eq!(b.closed_deals(ALICE)[0].pnl, 1000);
}

// ---- Персистентность (ADR-016) --------------------------------------------

#[test]
fn check_reports_only_changed_accounts() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, Some(5_950_000), None).unwrap();

    // Цена движется, но ничего не сработало — сохранять нечего.
    assert!(b.check(&marks(1, 5_990_000)).is_empty());

    // Достигли SL — счёт изменился и попал в список на сохранение.
    assert_eq!(b.check(&marks(1, 5_950_000)), vec![ALICE]);

    // Позиций больше нет — следующий тик снова пустой.
    assert!(b.check(&marks(1, 5_900_000)).is_empty());
}

#[test]
fn check_reports_account_on_pending_fill() {
    let mut b = Broker::new(1_000_000, 1);
    b.place_pending(ALICE, 1, PosSide::Long, 1000, 5_900_000, PD, QD, None, None);
    assert!(b.check(&marks(1, 6_000_000)).is_empty()); // выше лимита — не сработал
    assert_eq!(b.check(&marks(1, 5_900_000)), vec![ALICE]); // сработал
    assert_eq!(b.positions(ALICE).len(), 1);
}

#[test]
fn snapshot_restore_roundtrip() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD, Some(5_950_000), None).unwrap();
    b.place_pending(ALICE, 2, PosSide::Short, 500, 7_000_000, PD, QD, None, None);
    b.check(&marks(1, 5_950_000)); // закрыли по SL → появилась история
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
