//! Интеграционные тесты matching engine.
//!
//! Покрывают: постановку в стакан, полное/частичное исполнение, приоритет цена→время,
//! лимит без скрещивания, рыночные заявки, IOC, отмену, валидацию и инвариант «книга не скрещена».

use exchange_core::{
    Command, Event, MatchingEngine, OrderId, OrderType, Price, Qty, RejectReason, Side, TimeInForce,
};

// ---- Хелперы --------------------------------------------------------------

fn limit(id: u64, side: Side, price: i64, qty: i64, tif: TimeInForce) -> Command {
    Command::PlaceOrder {
        id: OrderId(id),
        side,
        order_type: OrderType::Limit { price: Price(price) },
        qty: Qty(qty),
        tif,
    }
}

fn market(id: u64, side: Side, qty: i64) -> Command {
    Command::PlaceOrder {
        id: OrderId(id),
        side,
        order_type: OrderType::Market,
        qty: Qty(qty),
        tif: TimeInForce::Ioc, // рыночная всегда IOC по смыслу
    }
}

/// Считает совершённые сделки в списке событий как кортежи (price, qty, maker, taker).
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

// ---- Тесты ----------------------------------------------------------------

#[test]
fn limit_order_rests_on_empty_book() {
    let mut e = MatchingEngine::new();
    let ev = e.apply(limit(1, Side::Buy, 100, 10, TimeInForce::Gtc));

    assert_eq!(ev[0], Event::OrderAccepted { id: OrderId(1) });
    assert_eq!(ev[1], Event::OrderResting { id: OrderId(1), price: Price(100), qty: Qty(10) });
    assert!(trades(&ev).is_empty());
    assert_eq!(e.book().best_bid(), Some(Price(100)));
    assert_eq!(e.book().best_ask(), None);
}

#[test]
fn full_match_fills_both_orders() {
    let mut e = MatchingEngine::new();
    e.apply(limit(1, Side::Sell, 100, 10, TimeInForce::Gtc));
    let ev = e.apply(limit(2, Side::Buy, 100, 10, TimeInForce::Gtc));

    assert_eq!(trades(&ev), vec![(100, 10, 1, 2)]);
    assert!(ev.contains(&Event::OrderFilled { id: OrderId(1) })); // maker
    assert!(ev.contains(&Event::OrderFilled { id: OrderId(2) })); // taker
    assert_eq!(e.book().best_bid(), None);
    assert_eq!(e.book().best_ask(), None);
}

#[test]
fn partial_maker_fill_leaves_remainder_in_book() {
    let mut e = MatchingEngine::new();
    e.apply(limit(1, Side::Sell, 100, 10, TimeInForce::Gtc));
    let ev = e.apply(limit(2, Side::Buy, 100, 4, TimeInForce::Gtc));

    assert_eq!(trades(&ev), vec![(100, 4, 1, 2)]);
    assert!(ev.contains(&Event::OrderFilled { id: OrderId(2) })); // taker добит
    assert!(!ev.contains(&Event::OrderFilled { id: OrderId(1) })); // maker остался
    assert_eq!(e.book().qty_at(Side::Sell, Price(100)), Qty(6));
}

#[test]
fn partial_taker_fill_rests_remainder() {
    let mut e = MatchingEngine::new();
    e.apply(limit(1, Side::Sell, 100, 4, TimeInForce::Gtc));
    let ev = e.apply(limit(2, Side::Buy, 100, 10, TimeInForce::Gtc));

    assert_eq!(trades(&ev), vec![(100, 4, 1, 2)]);
    assert!(ev.contains(&Event::OrderResting { id: OrderId(2), price: Price(100), qty: Qty(6) }));
    assert_eq!(e.book().best_ask(), None);
    assert_eq!(e.book().best_bid(), Some(Price(100)));
    assert_eq!(e.book().qty_at(Side::Buy, Price(100)), Qty(6));
}

#[test]
fn time_priority_within_price_level_is_fifo() {
    let mut e = MatchingEngine::new();
    e.apply(limit(1, Side::Sell, 100, 5, TimeInForce::Gtc)); // пришёл раньше
    e.apply(limit(2, Side::Sell, 100, 5, TimeInForce::Gtc)); // позже
    let ev = e.apply(limit(3, Side::Buy, 100, 5, TimeInForce::Gtc));

    // Матчимся против заявки 1 (FIFO), не против 2.
    assert_eq!(trades(&ev), vec![(100, 5, 1, 3)]);
    assert_eq!(e.book().qty_at(Side::Sell, Price(100)), Qty(5)); // осталась заявка 2
}

#[test]
fn price_priority_matches_best_price_first() {
    let mut e = MatchingEngine::new();
    e.apply(limit(1, Side::Sell, 101, 5, TimeInForce::Gtc));
    e.apply(limit(2, Side::Sell, 100, 5, TimeInForce::Gtc)); // лучшая цена
    let ev = e.apply(limit(3, Side::Buy, 105, 5, TimeInForce::Gtc));

    // Сначала исполняется дешёвая продажа (100), затем при остатке — 101.
    assert_eq!(trades(&ev), vec![(100, 5, 2, 3)]);
    assert_eq!(e.book().best_ask(), Some(Price(101)));
}

#[test]
fn limit_that_does_not_cross_just_rests() {
    let mut e = MatchingEngine::new();
    e.apply(limit(1, Side::Sell, 101, 10, TimeInForce::Gtc));
    let ev = e.apply(limit(2, Side::Buy, 100, 5, TimeInForce::Gtc)); // 100 < 101 — не скрещивается

    assert!(trades(&ev).is_empty());
    assert!(ev.contains(&Event::OrderResting { id: OrderId(2), price: Price(100), qty: Qty(5) }));
    assert!(!e.book().is_crossed());
}

#[test]
fn market_order_sweeps_multiple_levels() {
    let mut e = MatchingEngine::new();
    e.apply(limit(1, Side::Sell, 100, 5, TimeInForce::Gtc));
    e.apply(limit(2, Side::Sell, 101, 5, TimeInForce::Gtc));
    let ev = e.apply(market(3, Side::Buy, 8));

    assert_eq!(trades(&ev), vec![(100, 5, 1, 3), (101, 3, 2, 3)]);
    assert!(ev.contains(&Event::OrderFilled { id: OrderId(3) }));
    assert_eq!(e.book().qty_at(Side::Sell, Price(101)), Qty(2));
}

#[test]
fn market_order_without_liquidity_cancels_remainder() {
    let mut e = MatchingEngine::new();
    let ev = e.apply(market(1, Side::Buy, 5));

    assert!(trades(&ev).is_empty());
    assert!(ev.contains(&Event::OrderCanceledRemainder { id: OrderId(1), qty: Qty(5) }));
    assert_eq!(e.book().best_bid(), None); // рыночная не встаёт в стакан
}

#[test]
fn ioc_limit_cancels_unfilled_remainder() {
    let mut e = MatchingEngine::new();
    e.apply(limit(1, Side::Sell, 100, 3, TimeInForce::Gtc));
    let ev = e.apply(limit(2, Side::Buy, 100, 10, TimeInForce::Ioc));

    assert_eq!(trades(&ev), vec![(100, 3, 1, 2)]);
    assert!(ev.contains(&Event::OrderCanceledRemainder { id: OrderId(2), qty: Qty(7) }));
    assert_eq!(e.book().best_bid(), None); // остаток НЕ встал в стакан
}

#[test]
fn cancel_removes_resting_order() {
    let mut e = MatchingEngine::new();
    e.apply(limit(1, Side::Buy, 100, 5, TimeInForce::Gtc));
    let ev = e.apply(Command::CancelOrder { id: OrderId(1) });

    assert_eq!(ev, vec![Event::OrderCanceled { id: OrderId(1) }]);
    assert_eq!(e.book().best_bid(), None);
}

#[test]
fn cancel_unknown_order_is_rejected() {
    let mut e = MatchingEngine::new();
    let ev = e.apply(Command::CancelOrder { id: OrderId(99) });

    assert_eq!(
        ev,
        vec![Event::OrderRejected { id: OrderId(99), reason: RejectReason::UnknownOrder }]
    );
}

#[test]
fn non_positive_qty_is_rejected() {
    let mut e = MatchingEngine::new();
    let ev = e.apply(limit(1, Side::Buy, 100, 0, TimeInForce::Gtc));

    assert_eq!(
        ev,
        vec![Event::OrderRejected { id: OrderId(1), reason: RejectReason::NonPositiveQty }]
    );
}

#[test]
fn book_never_stays_crossed_after_matching() {
    let mut e = MatchingEngine::new();
    // Набор пересекающихся заявок; после каждого шага книга не должна быть скрещена.
    let cmds = [
        limit(1, Side::Buy, 100, 5, TimeInForce::Gtc),
        limit(2, Side::Buy, 101, 5, TimeInForce::Gtc),
        limit(3, Side::Sell, 100, 8, TimeInForce::Gtc),
        limit(4, Side::Sell, 99, 3, TimeInForce::Gtc),
        limit(5, Side::Buy, 102, 10, TimeInForce::Gtc),
    ];
    for c in cmds {
        e.apply(c);
        assert!(!e.book().is_crossed(), "книга оказалась скрещена — баг матчинга");
    }
}

#[test]
fn engine_is_deterministic() {
    // Один и тот же вход → идентичный поток событий (ADR-003).
    let script = || {
        let mut e = MatchingEngine::new();
        let mut all = Vec::new();
        for c in [
            limit(1, Side::Sell, 100, 5, TimeInForce::Gtc),
            limit(2, Side::Sell, 101, 5, TimeInForce::Gtc),
            market(3, Side::Buy, 7),
            limit(4, Side::Buy, 99, 4, TimeInForce::Gtc),
            Command::CancelOrder { id: OrderId(4) },
        ] {
            all.extend(e.apply(c));
        }
        all
    };
    assert_eq!(script(), script());
}
