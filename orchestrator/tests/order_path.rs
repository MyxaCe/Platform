//! Тесты полного пути ордера: резерв → матчинг → расчёт, с проверкой балансов и сходимости.

use domain::account::UserId;
use domain::instrument::{AssetId, Instrument, InstrumentId};
use domain::money::{Amount, Price, Qty};
use domain::order::{Side, TimeInForce};

use orchestrator::{OrderReject, Orchestrator};

const BTC: AssetId = AssetId(1);
const USD: AssetId = AssetId(2);
const INST: InstrumentId = InstrumentId(1);
const ALICE: UserId = UserId(1);
const BOB: UserId = UserId(2);

fn amt(v: i128) -> Amount {
    Amount(v)
}

/// Оркестратор с инструментом BTC-USD (tick/lot/min по умолчанию = 1, либо заданные).
fn setup(tick: i64, lot: i64, min: i64) -> Orchestrator {
    let mut o = Orchestrator::new();
    o.register_instrument(Instrument {
        id: INST,
        symbol: "BTC-USD".to_string(),
        base: BTC,
        quote: USD,
        price_decimals: 2,
        qty_decimals: 2,
        tick_size: Price(tick),
        lot_size: Qty(lot),
        min_qty: Qty(min),
    });
    o
}

fn buy(o: &mut Orchestrator, u: UserId, id: u64, price: i64, qty: i64) -> Result<(), OrderReject> {
    o.place_limit(u, INST, domain::order::OrderId(id), Side::Buy, Price(price), Qty(qty), TimeInForce::Gtc).map(|_| ())
}
fn sell(o: &mut Orchestrator, u: UserId, id: u64, price: i64, qty: i64) -> Result<(), OrderReject> {
    o.place_limit(u, INST, domain::order::OrderId(id), Side::Sell, Price(price), Qty(qty), TimeInForce::Gtc).map(|_| ())
}
fn market_buy(o: &mut Orchestrator, u: UserId, id: u64, qty: i64) -> Result<(), OrderReject> {
    o.place_market(u, INST, domain::order::OrderId(id), Side::Buy, Qty(qty)).map(|_| ())
}
fn market_sell(o: &mut Orchestrator, u: UserId, id: u64, qty: i64) -> Result<(), OrderReject> {
    o.place_market(u, INST, domain::order::OrderId(id), Side::Sell, Qty(qty)).map(|_| ())
}

// ---- Резерв ---------------------------------------------------------------

#[test]
fn sell_reserves_base_and_rests() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(10));
    sell(&mut o, ALICE, 1, 100, 5).unwrap();

    assert_eq!(o.held(ALICE, BTC), amt(5));
    assert_eq!(o.available(ALICE, BTC), amt(5));
    assert_eq!(o.book(INST).unwrap().best_ask(), Some(Price(100)));
}

#[test]
fn buy_reserves_quote_notional() {
    let mut o = setup(1, 1, 1);
    o.deposit(BOB, USD, amt(1000));
    buy(&mut o, BOB, 1, 100, 5).unwrap(); // резерв 500

    assert_eq!(o.held(BOB, USD), amt(500));
    assert_eq!(o.available(BOB, USD), amt(500));
}

#[test]
fn insufficient_funds_is_rejected_and_engine_untouched() {
    let mut o = setup(1, 1, 1);
    o.deposit(BOB, USD, amt(100));
    assert_eq!(buy(&mut o, BOB, 1, 100, 5), Err(OrderReject::InsufficientFunds)); // нужно 500

    assert_eq!(o.held(BOB, USD), Amount::ZERO);
    assert_eq!(o.available(BOB, USD), amt(100));
    assert!(o.book(INST).unwrap().best_bid().is_none()); // в движок не попало
}

// ---- Полный путь и расчёт -------------------------------------------------

#[test]
fn full_match_moves_money_between_users() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(10));
    o.deposit(BOB, USD, amt(1000));

    sell(&mut o, ALICE, 1, 100, 5).unwrap();
    buy(&mut o, BOB, 2, 100, 5).unwrap();

    // Alice: -5 BTC, +500 USD; Bob: +5 BTC, -500 USD.
    assert_eq!(o.available(ALICE, BTC), amt(5));
    assert_eq!(o.held(ALICE, BTC), Amount::ZERO);
    assert_eq!(o.available(ALICE, USD), amt(500));
    assert_eq!(o.available(BOB, BTC), amt(5));
    assert_eq!(o.available(BOB, USD), amt(500));
    assert_eq!(o.held(BOB, USD), Amount::ZERO);
    // Книга пуста, сходимость.
    assert!(o.book(INST).unwrap().best_bid().is_none());
    assert!(o.book(INST).unwrap().best_ask().is_none());
    assert_eq!(o.total_supply(USD), amt(1000));
    assert_eq!(o.total_supply(BTC), amt(10));
}

#[test]
fn taker_buy_gets_price_improvement_refund() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(5));
    o.deposit(BOB, USD, amt(1000));

    sell(&mut o, ALICE, 1, 100, 5).unwrap(); // maker по 100
    buy(&mut o, BOB, 2, 105, 5).unwrap(); // taker лимит 105 → исполнение по 100, возврат 25

    assert_eq!(o.available(BOB, USD), amt(500)); // 1000 - 500 факт
    assert_eq!(o.held(BOB, USD), Amount::ZERO);
    assert_eq!(o.available(BOB, BTC), amt(5));
    assert_eq!(o.available(ALICE, USD), amt(500));
    assert_eq!(o.total_supply(USD), amt(1000));
}

#[test]
fn partial_taker_fill_keeps_remainder_reserved() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(4));
    o.deposit(BOB, USD, amt(1000));

    sell(&mut o, ALICE, 1, 100, 4).unwrap();
    buy(&mut o, BOB, 2, 100, 10).unwrap(); // 4 исполнено, 6 стоит в стакане

    // Bob: за 6@100 держится 600, потратил 400 на BTC.
    assert_eq!(o.available(BOB, BTC), amt(4));
    assert_eq!(o.held(BOB, USD), amt(600));
    assert_eq!(o.available(BOB, USD), Amount::ZERO);
    assert_eq!(o.book(INST).unwrap().best_bid(), Some(Price(100)));
    assert_eq!(o.available(ALICE, USD), amt(400));
    assert_eq!(o.total_supply(USD), amt(1000));
}

// ---- Отмена / отказ -------------------------------------------------------

#[test]
fn cancel_releases_reservation() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(10));
    sell(&mut o, ALICE, 1, 100, 5).unwrap();
    assert_eq!(o.held(ALICE, BTC), amt(5));

    o.cancel(ALICE, INST, domain::order::OrderId(1)).unwrap();
    assert_eq!(o.held(ALICE, BTC), Amount::ZERO);
    assert_eq!(o.available(ALICE, BTC), amt(10));
    assert!(o.book(INST).unwrap().best_ask().is_none());
}

#[test]
fn engine_rejection_releases_reservation() {
    let mut o = setup(5, 1, 1); // tick = 5
    o.deposit(BOB, USD, amt(1000));
    // цена 103 не кратна 5 → движок отклонит, резерв должен вернуться
    let r = buy(&mut o, BOB, 1, 103, 5);
    assert_eq!(r, Err(OrderReject::Engine(exchange_core::RejectReason::PriceNotOnTick)));
    assert_eq!(o.held(BOB, USD), Amount::ZERO);
    assert_eq!(o.available(BOB, USD), amt(1000));
}

#[test]
fn cannot_cancel_another_users_order() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(10));
    sell(&mut o, ALICE, 1, 100, 5).unwrap();

    assert_eq!(o.cancel(BOB, INST, domain::order::OrderId(1)), Err(OrderReject::NotOwner));
    assert_eq!(o.held(ALICE, BTC), amt(5)); // резерв не тронут
    assert!(o.book(INST).unwrap().best_ask().is_some()); // заявка на месте
}

// ---- Рыночные заявки ------------------------------------------------------

#[test]
fn market_sell_matches_resting_bid() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(10));
    o.deposit(BOB, USD, amt(500));

    buy(&mut o, BOB, 1, 100, 5).unwrap(); // maker bid
    market_sell(&mut o, ALICE, 2, 5).unwrap(); // taker market sell

    assert_eq!(o.available(ALICE, USD), amt(500));
    assert_eq!(o.available(ALICE, BTC), amt(5));
    assert_eq!(o.available(BOB, BTC), amt(5));
    assert_eq!(o.held(BOB, USD), Amount::ZERO);
    assert_eq!(o.total_supply(USD), amt(500));
    assert_eq!(o.total_supply(BTC), amt(10));
}

#[test]
fn market_buy_sweeps_levels_with_exact_cost() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(10));
    o.deposit(BOB, USD, amt(1000));

    sell(&mut o, ALICE, 1, 100, 5).unwrap();
    sell(&mut o, ALICE, 2, 101, 5).unwrap();
    market_buy(&mut o, BOB, 3, 8).unwrap(); // 5@100 + 3@101 = 803

    assert_eq!(o.available(BOB, USD), amt(197)); // 1000 - 803
    assert_eq!(o.held(BOB, USD), Amount::ZERO);
    assert_eq!(o.available(BOB, BTC), amt(8));
    assert_eq!(o.available(ALICE, USD), amt(803));
    assert_eq!(o.book(INST).unwrap().qty_at(Side::Sell, Price(101)), Qty(2));
    assert_eq!(o.total_supply(USD), amt(1000));
    assert_eq!(o.total_supply(BTC), amt(10));
}

#[test]
fn market_buy_insufficient_funds_is_rejected() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(10));
    o.deposit(BOB, USD, amt(300));
    sell(&mut o, ALICE, 1, 100, 5).unwrap();

    assert_eq!(market_buy(&mut o, BOB, 2, 5), Err(OrderReject::InsufficientFunds)); // нужно 500
    assert_eq!(o.available(BOB, USD), amt(300));
    assert_eq!(o.held(BOB, USD), Amount::ZERO);
}

#[test]
fn market_buy_partial_on_thin_liquidity() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(3));
    o.deposit(BOB, USD, amt(1000));
    sell(&mut o, ALICE, 1, 100, 3).unwrap();

    market_buy(&mut o, BOB, 2, 5).unwrap(); // ликвидности только 3

    assert_eq!(o.available(BOB, BTC), amt(3));
    assert_eq!(o.available(BOB, USD), amt(700)); // потрачено 300
    assert_eq!(o.held(BOB, USD), Amount::ZERO); // остаток резерва не повис
    assert_eq!(o.total_supply(USD), amt(1000));
}

// ---- Self-trade prevention ------------------------------------------------

#[test]
fn self_trade_limit_is_rejected() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(10));
    o.deposit(ALICE, USD, amt(1000));
    sell(&mut o, ALICE, 1, 100, 5).unwrap();

    // Своя же покупка по 100 пересеклась бы со своей продажей.
    assert_eq!(buy(&mut o, ALICE, 2, 100, 5), Err(OrderReject::SelfTrade));
    // Продажа на месте, лишнего резерва нет.
    assert_eq!(o.held(ALICE, BTC), amt(5));
    assert_eq!(o.held(ALICE, USD), Amount::ZERO);
    assert!(o.book(INST).unwrap().best_ask().is_some());
}

#[test]
fn non_crossing_own_order_is_allowed() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(10));
    o.deposit(ALICE, USD, amt(1000));
    sell(&mut o, ALICE, 1, 101, 5).unwrap();

    // Покупка по 100 не пересекается со своей продажей по 101 — разрешено.
    assert_eq!(buy(&mut o, ALICE, 2, 100, 5), Ok(()));
    assert_eq!(o.book(INST).unwrap().best_bid(), Some(Price(100)));
}

#[test]
fn self_trade_market_is_rejected() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(10));
    o.deposit(ALICE, USD, amt(1000));
    sell(&mut o, ALICE, 1, 100, 5).unwrap();

    assert_eq!(market_buy(&mut o, ALICE, 2, 5), Err(OrderReject::SelfTrade));
    assert_eq!(o.held(ALICE, USD), Amount::ZERO);
}

#[test]
fn conservation_holds_across_full_scenario() {
    let mut o = setup(1, 1, 1);
    o.deposit(ALICE, BTC, amt(50));
    o.deposit(BOB, USD, amt(10_000));

    sell(&mut o, ALICE, 1, 100, 10).unwrap();
    sell(&mut o, ALICE, 2, 101, 10).unwrap();
    buy(&mut o, BOB, 3, 105, 15).unwrap(); // сметает 10@100 + 5@101, остаток 0? нет: 15 исполнено
    buy(&mut o, BOB, 4, 99, 5).unwrap(); // стоит в стакане
    o.cancel(BOB, INST, domain::order::OrderId(4)).unwrap();

    // Полная сходимость по обоим активам.
    assert_eq!(o.total_supply(USD), amt(10_000));
    assert_eq!(o.total_supply(BTC), amt(50));
}
