//! Тесты Ledger: депозит/вывод, резерв/возврат, расчёт сделки, инвариант сходимости.

use domain::account::UserId;
use domain::instrument::AssetId;
use domain::money::{Amount, Price, Qty};
use ledger::{Ledger, LedgerError};

const BTC: AssetId = AssetId(1);
const USD: AssetId = AssetId(2);
const ALICE: UserId = UserId(1);
const BOB: UserId = UserId(2);

fn amt(v: i128) -> Amount {
    Amount(v)
}

// ---- Депозит / вывод ------------------------------------------------------

#[test]
fn deposit_increases_available_and_supply() {
    let mut l = Ledger::new();
    l.deposit(ALICE, USD, amt(1000));
    assert_eq!(l.available(ALICE, USD), amt(1000));
    assert_eq!(l.total_supply(USD), amt(1000));
}

#[test]
fn withdraw_insufficient_is_rejected_and_state_unchanged() {
    let mut l = Ledger::new();
    l.deposit(ALICE, USD, amt(100));
    assert_eq!(l.withdraw(ALICE, USD, amt(150)), Err(LedgerError::InsufficientAvailable));
    assert_eq!(l.available(ALICE, USD), amt(100)); // не изменилось
}

#[test]
fn withdraw_sufficient_reduces_supply() {
    let mut l = Ledger::new();
    l.deposit(ALICE, USD, amt(100));
    assert_eq!(l.withdraw(ALICE, USD, amt(40)), Ok(()));
    assert_eq!(l.available(ALICE, USD), amt(60));
    assert_eq!(l.total_supply(USD), amt(60));
}

// ---- Резерв / возврат -----------------------------------------------------

#[test]
fn reserve_moves_available_to_held_and_conserves_total() {
    let mut l = Ledger::new();
    l.deposit(ALICE, USD, amt(1000));
    assert_eq!(l.reserve(ALICE, USD, amt(300)), Ok(()));
    assert_eq!(l.available(ALICE, USD), amt(700));
    assert_eq!(l.held(ALICE, USD), amt(300));
    assert_eq!(l.total_supply(USD), amt(1000)); // резерв не создаёт/не жжёт средства
}

#[test]
fn reserve_insufficient_is_rejected() {
    let mut l = Ledger::new();
    l.deposit(ALICE, USD, amt(100));
    assert_eq!(l.reserve(ALICE, USD, amt(200)), Err(LedgerError::InsufficientAvailable));
    assert_eq!(l.available(ALICE, USD), amt(100));
    assert_eq!(l.held(ALICE, USD), Amount::ZERO);
}

#[test]
fn release_moves_held_back_to_available() {
    let mut l = Ledger::new();
    l.deposit(ALICE, USD, amt(1000));
    l.reserve(ALICE, USD, amt(300)).unwrap();
    assert_eq!(l.release(ALICE, USD, amt(300)), Ok(()));
    assert_eq!(l.available(ALICE, USD), amt(1000));
    assert_eq!(l.held(ALICE, USD), Amount::ZERO);
}

#[test]
fn release_more_than_held_is_rejected() {
    let mut l = Ledger::new();
    l.deposit(ALICE, USD, amt(1000));
    l.reserve(ALICE, USD, amt(100)).unwrap();
    assert_eq!(l.release(ALICE, USD, amt(200)), Err(LedgerError::InsufficientHeld));
}

// ---- Расчёт сделки --------------------------------------------------------

#[test]
fn settle_fill_at_reserve_price_moves_assets_and_conserves() {
    // Alice покупает 5 BTC по 100 (резерв по 100), Bob продаёт 5 BTC.
    let mut l = Ledger::new();
    l.deposit(ALICE, USD, amt(1000));
    l.deposit(BOB, BTC, amt(5));
    l.reserve(ALICE, USD, amt(500)).unwrap(); // 100 * 5
    l.reserve(BOB, BTC, amt(5)).unwrap();

    l.settle_fill(BTC, USD, ALICE, BOB, Price(100), Qty(5), Price(100)).unwrap();

    // Alice: заплатила 500 USD, получила 5 BTC.
    assert_eq!(l.available(ALICE, USD), amt(500));
    assert_eq!(l.held(ALICE, USD), Amount::ZERO);
    assert_eq!(l.available(ALICE, BTC), amt(5));
    // Bob: отдал 5 BTC, получил 500 USD.
    assert_eq!(l.held(BOB, BTC), Amount::ZERO);
    assert_eq!(l.available(BOB, USD), amt(500));
    // Сходимость по обоим активам.
    assert_eq!(l.total_supply(USD), amt(1000));
    assert_eq!(l.total_supply(BTC), amt(5));
}

#[test]
fn settle_fill_refunds_price_improvement() {
    // Alice — тейкер с лимитом 105, исполнение по 100: разница 5*5=25 возвращается.
    let mut l = Ledger::new();
    l.deposit(ALICE, USD, amt(525));
    l.deposit(BOB, BTC, amt(5));
    l.reserve(ALICE, USD, amt(525)).unwrap(); // 105 * 5
    l.reserve(BOB, BTC, amt(5)).unwrap();

    l.settle_fill(BTC, USD, ALICE, BOB, Price(100), Qty(5), Price(105)).unwrap();

    // Alice заплатила фактически 500, 25 вернулись в available.
    assert_eq!(l.available(ALICE, USD), amt(25));
    assert_eq!(l.held(ALICE, USD), Amount::ZERO);
    assert_eq!(l.available(ALICE, BTC), amt(5));
    assert_eq!(l.available(BOB, USD), amt(500));
    // Сходимость.
    assert_eq!(l.total_supply(USD), amt(525));
    assert_eq!(l.total_supply(BTC), amt(5));
}

#[test]
fn settle_fill_without_reserve_is_rejected() {
    let mut l = Ledger::new();
    l.deposit(ALICE, USD, amt(1000)); // не зарезервировано
    l.deposit(BOB, BTC, amt(5));
    l.reserve(BOB, BTC, amt(5)).unwrap();
    assert_eq!(
        l.settle_fill(BTC, USD, ALICE, BOB, Price(100), Qty(5), Price(100)),
        Err(LedgerError::InsufficientHeld)
    );
}

#[test]
fn conservation_holds_across_many_operations() {
    // Серия резервов/сделок не меняет total_supply от депозитов.
    let mut l = Ledger::new();
    l.deposit(ALICE, USD, amt(10_000));
    l.deposit(BOB, BTC, amt(100));

    for _ in 0..10 {
        l.reserve(ALICE, USD, amt(500)).unwrap();
        l.reserve(BOB, BTC, amt(5)).unwrap();
        l.settle_fill(BTC, USD, ALICE, BOB, Price(100), Qty(5), Price(100)).unwrap();
        // Обратная сделка, чтобы у Alice снова были USD, а у Bob — BTC.
        l.reserve(ALICE, BTC, amt(5)).unwrap();
        l.reserve(BOB, USD, amt(500)).unwrap();
        l.settle_fill(BTC, USD, BOB, ALICE, Price(100), Qty(5), Price(100)).unwrap();
    }

    assert_eq!(l.total_supply(USD), amt(10_000));
    assert_eq!(l.total_supply(BTC), amt(100));
}
