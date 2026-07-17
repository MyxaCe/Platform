//! Тесты бумажного брокера: маржа, P&L, закрытие, equity.

use std::collections::HashMap;

use broker::{Broker, BrokerError, PosSide};
use domain::account::UserId;

const ALICE: UserId = UserId(1);

// BTC: pd=2, qd=5. Цена 60000.00 → raw 6_000_000. Объём 0.01 → raw 1000.
// notional = 6_000_000 * 1000 * 100 / 10^7 = 60_000 центов = $600.
const PD: u8 = 2;
const QD: u8 = 5;

fn marks(inst: u32, price: i64) -> HashMap<u32, i64> {
    HashMap::from([(inst, price)])
}

#[test]
fn open_locks_margin_and_reduces_free() {
    let mut b = Broker::new(1_000_000, 1); // $10k, leverage 1
    let id = b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD).unwrap();
    assert_eq!(id, 1);
    assert_eq!(b.used_margin(ALICE), 60_000); // $600
    assert_eq!(b.free_margin(ALICE), 1_000_000 - 60_000);
    assert_eq!(b.balance(ALICE), 1_000_000); // баланс не тронут (маржа — блокировка)
}

#[test]
fn insufficient_margin_is_rejected() {
    let mut b = Broker::new(50_000, 1); // $500 — мало под $600
    assert_eq!(b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD), Err(BrokerError::InsufficientMargin));
}

#[test]
fn long_pnl_is_positive_when_price_rises() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD).unwrap();
    // цена 60000 → 61000 (+1000.00 = raw +100000); PnL = 100000*1000*100/1e7 = 1000 центов = +$10
    assert_eq!(b.open_pnl(ALICE, &marks(1, 6_100_000)), 1000);
    assert_eq!(b.equity(ALICE, &marks(1, 6_100_000)), 1_001_000);
}

#[test]
fn short_pnl_is_positive_when_price_falls() {
    let mut b = Broker::new(1_000_000, 1);
    b.open(ALICE, 1, PosSide::Short, 1000, 6_000_000, PD, QD).unwrap();
    // цена падает 60000 → 59000: short в плюсе на $10
    assert_eq!(b.open_pnl(ALICE, &marks(1, 5_900_000)), 1000);
    // цена растёт → short в минусе
    assert_eq!(b.open_pnl(ALICE, &marks(1, 6_100_000)), -1000);
}

#[test]
fn close_realizes_pnl_into_balance() {
    let mut b = Broker::new(1_000_000, 1);
    let id = b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD).unwrap();
    let pnl = b.close(ALICE, id, 6_100_000).unwrap();
    assert_eq!(pnl, 1000); // +$10
    assert_eq!(b.balance(ALICE), 1_001_000);
    assert!(b.positions(ALICE).is_empty());
    assert_eq!(b.used_margin(ALICE), 0);
}

#[test]
fn losing_trade_reduces_balance() {
    let mut b = Broker::new(1_000_000, 1);
    let id = b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD).unwrap();
    let pnl = b.close(ALICE, id, 5_900_000).unwrap(); // цена упала
    assert_eq!(pnl, -1000);
    assert_eq!(b.balance(ALICE), 999_000); // проиграл $10
}

#[test]
fn close_unknown_position_is_rejected() {
    let mut b = Broker::new(1_000_000, 1);
    assert_eq!(b.close(ALICE, 99, 6_000_000), Err(BrokerError::UnknownPosition));
}

#[test]
fn free_margin_frees_after_close() {
    let mut b = Broker::new(1_000_000, 1);
    let id = b.open(ALICE, 1, PosSide::Long, 1000, 6_000_000, PD, QD).unwrap();
    assert_eq!(b.free_margin(ALICE), 940_000);
    b.close(ALICE, id, 6_000_000).unwrap();
    assert_eq!(b.free_margin(ALICE), 1_000_000);
}
