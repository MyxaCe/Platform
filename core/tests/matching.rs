//! Интеграционные тесты matching engine.
//!
//! Покрывают: постановку в стакан, полное/частичное исполнение, приоритет цена→время,
//! лимит без скрещивания, рыночные заявки, IOC, отмену, валидацию (инструмент/tick/lot/min),
//! мульти-инструментную изоляцию, инвариант «книга не скрещена» и детерминизм.

use exchange_core::{
    AssetId, Command, Event, Instrument, InstrumentId, MatchingEngine, OrderId, OrderType, Price,
    Qty, RejectReason, Side, TimeInForce,
};

// ---- Хелперы --------------------------------------------------------------

/// Инструмент по умолчанию: tick=1, lot=1, min=1 → любые целые цены/объёмы валидны.
const I1: InstrumentId = InstrumentId(1);

fn instrument(id: InstrumentId, symbol: &str, tick: i64, lot: i64, min: i64) -> Instrument {
    Instrument {
        id,
        symbol: symbol.to_string(),
        base: AssetId(1),
        quote: AssetId(2),
        price_decimals: 2,
        qty_decimals: 2,
        tick_size: Price(tick),
        lot_size: Qty(lot),
        min_qty: Qty(min),
    }
}

/// Движок с одним инструментом I1 (tick=lot=min=1).
fn engine() -> MatchingEngine {
    let mut e = MatchingEngine::new();
    e.register_instrument(instrument(I1, "TEST-USD", 1, 1, 1));
    e
}

fn limit(id: u64, side: Side, price: i64, qty: i64, tif: TimeInForce) -> Command {
    Command::PlaceOrder {
        instrument: I1,
        id: OrderId(id),
        side,
        order_type: OrderType::Limit { price: Price(price) },
        qty: Qty(qty),
        tif,
    }
}

fn market(id: u64, side: Side, qty: i64) -> Command {
    Command::PlaceOrder {
        instrument: I1,
        id: OrderId(id),
        side,
        order_type: OrderType::Market,
        qty: Qty(qty),
        tif: TimeInForce::Ioc, // рыночная всегда IOC по смыслу
    }
}

/// Считает совершённые сделки как кортежи (price, qty, maker, taker).
fn trades(events: &[Event]) -> Vec<(i64, i64, u64, u64)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Trade { price, qty, maker, taker, .. } => {
                Some((price.0, qty.0, maker.0, taker.0))
            }
            _ => None,
        })
        .collect()
}

/// Единственная причина отказа в списке событий (для тестов валидации).
fn reject_reason(events: &[Event]) -> Option<RejectReason> {
    events.iter().find_map(|e| match e {
        Event::OrderRejected { reason, .. } => Some(*reason),
        _ => None,
    })
}

// ---- Матчинг --------------------------------------------------------------

#[test]
fn limit_order_rests_on_empty_book() {
    let mut e = engine();
    let ev = e.apply(limit(1, Side::Buy, 100, 10, TimeInForce::Gtc));

    assert_eq!(ev[0], Event::OrderAccepted { instrument: I1, id: OrderId(1) });
    assert_eq!(ev[1], Event::OrderResting { instrument: I1, id: OrderId(1), price: Price(100), qty: Qty(10) });
    assert!(trades(&ev).is_empty());
    assert_eq!(e.book(I1).unwrap().best_bid(), Some(Price(100)));
    assert_eq!(e.book(I1).unwrap().best_ask(), None);
}

#[test]
fn full_match_fills_both_orders() {
    let mut e = engine();
    e.apply(limit(1, Side::Sell, 100, 10, TimeInForce::Gtc));
    let ev = e.apply(limit(2, Side::Buy, 100, 10, TimeInForce::Gtc));

    assert_eq!(trades(&ev), vec![(100, 10, 1, 2)]);
    assert!(ev.contains(&Event::OrderFilled { instrument: I1, id: OrderId(1) })); // maker
    assert!(ev.contains(&Event::OrderFilled { instrument: I1, id: OrderId(2) })); // taker
    assert_eq!(e.book(I1).unwrap().best_bid(), None);
    assert_eq!(e.book(I1).unwrap().best_ask(), None);
}

#[test]
fn partial_maker_fill_leaves_remainder_in_book() {
    let mut e = engine();
    e.apply(limit(1, Side::Sell, 100, 10, TimeInForce::Gtc));
    let ev = e.apply(limit(2, Side::Buy, 100, 4, TimeInForce::Gtc));

    assert_eq!(trades(&ev), vec![(100, 4, 1, 2)]);
    assert!(ev.contains(&Event::OrderFilled { instrument: I1, id: OrderId(2) })); // taker добит
    assert!(!ev.contains(&Event::OrderFilled { instrument: I1, id: OrderId(1) })); // maker остался
    assert_eq!(e.book(I1).unwrap().qty_at(Side::Sell, Price(100)), Qty(6));
}

#[test]
fn partial_taker_fill_rests_remainder() {
    let mut e = engine();
    e.apply(limit(1, Side::Sell, 100, 4, TimeInForce::Gtc));
    let ev = e.apply(limit(2, Side::Buy, 100, 10, TimeInForce::Gtc));

    assert_eq!(trades(&ev), vec![(100, 4, 1, 2)]);
    assert!(ev.contains(&Event::OrderResting { instrument: I1, id: OrderId(2), price: Price(100), qty: Qty(6) }));
    assert_eq!(e.book(I1).unwrap().best_ask(), None);
    assert_eq!(e.book(I1).unwrap().best_bid(), Some(Price(100)));
    assert_eq!(e.book(I1).unwrap().qty_at(Side::Buy, Price(100)), Qty(6));
}

#[test]
fn time_priority_within_price_level_is_fifo() {
    let mut e = engine();
    e.apply(limit(1, Side::Sell, 100, 5, TimeInForce::Gtc)); // пришёл раньше
    e.apply(limit(2, Side::Sell, 100, 5, TimeInForce::Gtc)); // позже
    let ev = e.apply(limit(3, Side::Buy, 100, 5, TimeInForce::Gtc));

    assert_eq!(trades(&ev), vec![(100, 5, 1, 3)]); // FIFO: заявка 1
    assert_eq!(e.book(I1).unwrap().qty_at(Side::Sell, Price(100)), Qty(5)); // осталась заявка 2
}

#[test]
fn price_priority_matches_best_price_first() {
    let mut e = engine();
    e.apply(limit(1, Side::Sell, 101, 5, TimeInForce::Gtc));
    e.apply(limit(2, Side::Sell, 100, 5, TimeInForce::Gtc)); // лучшая цена
    let ev = e.apply(limit(3, Side::Buy, 105, 5, TimeInForce::Gtc));

    assert_eq!(trades(&ev), vec![(100, 5, 2, 3)]);
    assert_eq!(e.book(I1).unwrap().best_ask(), Some(Price(101)));
}

#[test]
fn limit_that_does_not_cross_just_rests() {
    let mut e = engine();
    e.apply(limit(1, Side::Sell, 101, 10, TimeInForce::Gtc));
    let ev = e.apply(limit(2, Side::Buy, 100, 5, TimeInForce::Gtc)); // 100 < 101 — не скрещивается

    assert!(trades(&ev).is_empty());
    assert!(ev.contains(&Event::OrderResting { instrument: I1, id: OrderId(2), price: Price(100), qty: Qty(5) }));
    assert!(!e.book(I1).unwrap().is_crossed());
}

#[test]
fn market_order_sweeps_multiple_levels() {
    let mut e = engine();
    e.apply(limit(1, Side::Sell, 100, 5, TimeInForce::Gtc));
    e.apply(limit(2, Side::Sell, 101, 5, TimeInForce::Gtc));
    let ev = e.apply(market(3, Side::Buy, 8));

    assert_eq!(trades(&ev), vec![(100, 5, 1, 3), (101, 3, 2, 3)]);
    assert!(ev.contains(&Event::OrderFilled { instrument: I1, id: OrderId(3) }));
    assert_eq!(e.book(I1).unwrap().qty_at(Side::Sell, Price(101)), Qty(2));
}

#[test]
fn market_order_without_liquidity_cancels_remainder() {
    let mut e = engine();
    let ev = e.apply(market(1, Side::Buy, 5));

    assert!(trades(&ev).is_empty());
    assert!(ev.contains(&Event::OrderCanceledRemainder { instrument: I1, id: OrderId(1), qty: Qty(5) }));
    assert_eq!(e.book(I1).unwrap().best_bid(), None); // рыночная не встаёт в стакан
}

#[test]
fn ioc_limit_cancels_unfilled_remainder() {
    let mut e = engine();
    e.apply(limit(1, Side::Sell, 100, 3, TimeInForce::Gtc));
    let ev = e.apply(limit(2, Side::Buy, 100, 10, TimeInForce::Ioc));

    assert_eq!(trades(&ev), vec![(100, 3, 1, 2)]);
    assert!(ev.contains(&Event::OrderCanceledRemainder { instrument: I1, id: OrderId(2), qty: Qty(7) }));
    assert_eq!(e.book(I1).unwrap().best_bid(), None); // остаток НЕ встал в стакан
}

#[test]
fn cancel_removes_resting_order() {
    let mut e = engine();
    e.apply(limit(1, Side::Buy, 100, 5, TimeInForce::Gtc));
    let ev = e.apply(Command::CancelOrder { instrument: I1, id: OrderId(1) });

    assert_eq!(ev, vec![Event::OrderCanceled { instrument: I1, id: OrderId(1) }]);
    assert_eq!(e.book(I1).unwrap().best_bid(), None);
}

#[test]
fn cancel_unknown_order_is_rejected() {
    let mut e = engine();
    let ev = e.apply(Command::CancelOrder { instrument: I1, id: OrderId(99) });
    assert_eq!(reject_reason(&ev), Some(RejectReason::UnknownOrder));
}

// ---- Валидация (ADR-005) --------------------------------------------------

#[test]
fn non_positive_qty_is_rejected() {
    let mut e = engine();
    let ev = e.apply(limit(1, Side::Buy, 100, 0, TimeInForce::Gtc));
    assert_eq!(reject_reason(&ev), Some(RejectReason::NonPositiveQty));
}

#[test]
fn unknown_instrument_is_rejected() {
    let mut e = engine();
    let ev = e.apply(Command::PlaceOrder {
        instrument: InstrumentId(99),
        id: OrderId(1),
        side: Side::Buy,
        order_type: OrderType::Limit { price: Price(100) },
        qty: Qty(1),
        tif: TimeInForce::Gtc,
    });
    assert_eq!(reject_reason(&ev), Some(RejectReason::UnknownInstrument));
}

#[test]
fn qty_not_on_lot_is_rejected() {
    let mut e = MatchingEngine::new();
    e.register_instrument(instrument(I1, "LOT10", 5, 10, 10)); // lot=10
    let ev = e.apply(limit(1, Side::Buy, 100, 15, TimeInForce::Gtc)); // 15 не кратно 10
    assert_eq!(reject_reason(&ev), Some(RejectReason::QtyNotOnLot));
}

#[test]
fn below_min_qty_is_rejected() {
    let mut e = MatchingEngine::new();
    e.register_instrument(instrument(I1, "MIN20", 5, 10, 20)); // lot=10, min=20
    let ev = e.apply(limit(1, Side::Buy, 100, 10, TimeInForce::Gtc)); // кратно 10, но < 20
    assert_eq!(reject_reason(&ev), Some(RejectReason::BelowMinQty));
}

#[test]
fn price_not_on_tick_is_rejected() {
    let mut e = MatchingEngine::new();
    e.register_instrument(instrument(I1, "TICK5", 5, 10, 10)); // tick=5
    let ev = e.apply(limit(1, Side::Buy, 103, 10, TimeInForce::Gtc)); // 103 не кратно 5
    assert_eq!(reject_reason(&ev), Some(RejectReason::PriceNotOnTick));
}

#[test]
fn valid_order_on_tick_and_lot_is_accepted() {
    let mut e = MatchingEngine::new();
    e.register_instrument(instrument(I1, "TICK5", 5, 10, 10));
    let ev = e.apply(limit(1, Side::Buy, 105, 20, TimeInForce::Gtc)); // 105%5==0, 20%10==0, 20>=10
    assert_eq!(reject_reason(&ev), None);
    assert!(ev.contains(&Event::OrderResting { instrument: I1, id: OrderId(1), price: Price(105), qty: Qty(20) }));
}

// ---- Мульти-инструментность -----------------------------------------------

#[test]
fn instruments_have_isolated_books() {
    let i2 = InstrumentId(2);
    let mut e = MatchingEngine::new();
    e.register_instrument(instrument(I1, "AAA-USD", 1, 1, 1));
    e.register_instrument(instrument(i2, "BBB-USD", 1, 1, 1));

    e.apply(limit(1, Side::Sell, 100, 5, TimeInForce::Gtc)); // в книге I1
    let ev = e.apply(Command::PlaceOrder {
        instrument: i2,
        id: OrderId(2),
        side: Side::Buy,
        order_type: OrderType::Limit { price: Price(100) },
        qty: Qty(5),
        tif: TimeInForce::Gtc,
    });

    // Заявка в I2 не должна исполниться против ликвидности I1.
    assert!(trades(&ev).is_empty());
    assert_eq!(e.book(I1).unwrap().qty_at(Side::Sell, Price(100)), Qty(5));
    assert_eq!(e.book(i2).unwrap().qty_at(Side::Buy, Price(100)), Qty(5));
}

// ---- Инварианты и детерминизм ---------------------------------------------

#[test]
fn book_never_stays_crossed_after_matching() {
    let mut e = engine();
    let cmds = [
        limit(1, Side::Buy, 100, 5, TimeInForce::Gtc),
        limit(2, Side::Buy, 101, 5, TimeInForce::Gtc),
        limit(3, Side::Sell, 100, 8, TimeInForce::Gtc),
        limit(4, Side::Sell, 99, 3, TimeInForce::Gtc),
        limit(5, Side::Buy, 102, 10, TimeInForce::Gtc),
    ];
    for c in cmds {
        e.apply(c);
        assert!(!e.book(I1).unwrap().is_crossed(), "книга оказалась скрещена — баг матчинга");
    }
}

#[test]
fn engine_is_deterministic() {
    let script = || {
        let mut e = engine();
        let mut all = Vec::new();
        for c in [
            limit(1, Side::Sell, 100, 5, TimeInForce::Gtc),
            limit(2, Side::Sell, 101, 5, TimeInForce::Gtc),
            market(3, Side::Buy, 7),
            limit(4, Side::Buy, 99, 4, TimeInForce::Gtc),
            Command::CancelOrder { instrument: I1, id: OrderId(4) },
        ] {
            all.extend(e.apply(c));
        }
        all
    };
    assert_eq!(script(), script());
}

#[test]
fn invariants_hold_under_random_load() {
    // Детерминированный фаззинг: фиксированный сид LCG (в тестах случайность допустима).
    // На каждом шаге проверяем, что книга не скрещена, ни при каких комбинациях заявок.
    let mut e = engine();
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut rng = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state >> 33
    };

    let mut next_id: u64 = 1;
    for _ in 0..5000 {
        if rng() % 5 == 0 && next_id > 1 {
            // Отмена случайной (возможно, уже несуществующей) заявки — это допустимо.
            let victim = 1 + rng() % (next_id - 1);
            e.apply(Command::CancelOrder { instrument: I1, id: OrderId(victim) });
        } else {
            let side = if rng() % 2 == 0 { Side::Buy } else { Side::Sell };
            let price = 90 + (rng() % 20) as i64; // 90..=109
            let qty = 1 + (rng() % 10) as i64; // 1..=10
            e.apply(limit(next_id, side, price, qty, TimeInForce::Gtc));
            next_id += 1;
        }
        assert!(!e.book(I1).unwrap().is_crossed(), "книга скрестилась под нагрузкой");
    }
}
