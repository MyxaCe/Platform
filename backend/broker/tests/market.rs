use std::collections::{HashMap, HashSet};

use broker::journal::{CloseCause, EventKind, RejectReason};
use broker::{Broker, MarketView, PosSide};
use domain::account::UserId;

const U: UserId = UserId(7);
const FX: u32 = 40;
const BTC: u32 = 1;

fn view<'a>(
    marks: &'a HashMap<u32, i64>,
    exec: &'a HashMap<u32, i64>,
    gap: &'a HashSet<u32>,
    close_only: &'a (dyn Fn(UserId, u32) -> bool + Sync),
    reject: bool,
) -> MarketView<'a> {
    MarketView {
        marks,
        exec,
        gap,
        close_only,
        reject_close_only: reject,
    }
}

fn never(_: UserId, _: u32) -> bool {
    false
}

#[test]
fn a_closed_market_does_not_fire_stops_or_orders_on_the_stale_price() {
    let mut b = Broker::new(10_000_000, 1);
    let none = HashMap::new();
    b.open(
        U,
        &none,
        FX,
        PosSide::Long,
        1000,
        108_000,
        5,
        0,
        Some(107_000),
        None,
    )
    .unwrap();
    b.place_pending(U, FX, PosSide::Long, 1000, 106_500, 5, 0, None, None)
        .unwrap();
    b.take_events(U);

    let marks = HashMap::from([(FX, 106_000)]);
    let exec = HashMap::new();
    let gap = HashSet::new();
    let changed = b.check_market(
        &view(&marks, &exec, &gap, &never, false),
        false,
        &HashSet::new(),
    );
    assert!(changed.is_empty());
    assert_eq!(b.positions(U).len(), 1);
    assert_eq!(b.pendings(U).len(), 1);
    assert!(b.take_events(U).is_empty());
}

#[test]
fn the_first_price_after_the_open_fires_at_the_gap_price_and_is_marked() {
    let mut b = Broker::new(10_000_000, 1);
    let none = HashMap::new();
    let id = b
        .open(
            U,
            &none,
            FX,
            PosSide::Long,
            1000,
            108_000,
            5,
            0,
            Some(107_000),
            None,
        )
        .unwrap();
    b.take_events(U);

    let marks = HashMap::from([(FX, 105_000), (BTC, 6_500_000)]);
    let gap = HashSet::from([FX]);
    b.check_market(
        &view(&marks, &marks, &gap, &never, false),
        false,
        &HashSet::new(),
    );
    let events = b.take_events(U);
    assert!(events.contains(&EventKind::PositionClosed {
        id,
        exit: 105_000,
        cause: CloseCause::StopLoss,
        level: Some(107_000),
        across_gap: true,
    }));
}

#[test]
fn close_only_holds_a_reached_order_by_default_and_rejects_it_under_the_flag() {
    let close_only = |_: UserId, i: u32| i == FX;
    let marks = HashMap::from([(FX, 106_000)]);
    let gap = HashSet::new();

    let mut b = Broker::new(10_000_000, 1);
    b.place_pending(U, FX, PosSide::Long, 1000, 106_500, 5, 0, None, None)
        .unwrap();
    b.take_events(U);
    let changed = b.check_market(
        &view(&marks, &marks, &gap, &close_only, false),
        false,
        &HashSet::new(),
    );
    assert!(changed.is_empty());
    assert_eq!(b.pendings(U).len(), 1);
    assert!(b.positions(U).is_empty());

    let changed = b.check_market(
        &view(&marks, &marks, &gap, &close_only, true),
        false,
        &HashSet::new(),
    );
    assert_eq!(changed, vec![U]);
    assert!(b.pendings(U).is_empty());
    assert!(b.positions(U).is_empty());
    let id = match b.take_events(U).as_slice() {
        [EventKind::PendingRejected {
            id,
            exec: Some(106_000),
            reason: RejectReason::InstrumentCloseOnly,
        }] => *id,
        other => panic!("ожидали отказ ордера по closeOnly, получили {other:?}"),
    };
    assert_eq!(id, 1);
}

#[test]
fn stop_out_values_the_account_with_stale_prices_but_closes_only_what_trades() {
    let mut b = Broker::new(20_000, 10).with_stop_out_pct(Some(50));
    let none = HashMap::new();
    b.open(U, &none, FX, PosSide::Long, 1000, 100_000, 5, 0, None, None)
        .unwrap();
    let btc = b
        .open(
            U,
            &none,
            BTC,
            PosSide::Long,
            100,
            6_000_000,
            2,
            5,
            None,
            None,
        )
        .unwrap();
    b.take_events(U);

    let marks = HashMap::from([(FX, 85_000), (BTC, 6_000_000)]);
    let exec = HashMap::from([(BTC, 6_000_000)]);
    let gap = HashSet::new();
    b.check_market(
        &view(&marks, &exec, &gap, &never, false),
        false,
        &HashSet::new(),
    );
    let events = b.take_events(U);
    assert!(events.iter().any(|e| matches!(
        e,
        EventKind::PositionClosed { id, cause: CloseCause::StopOut, .. } if *id == btc
    )));
    assert!(b.positions(U).iter().any(|p| p.instrument == FX));
}

#[test]
fn check_with_frozen_is_the_same_run_as_before() {
    let none = HashMap::new();
    let run = |use_market: bool| {
        let mut b = Broker::new(10_000_000, 1);
        b.open(
            U,
            &none,
            BTC,
            PosSide::Long,
            1000,
            6_000_000,
            2,
            5,
            Some(5_900_000),
            None,
        )
        .unwrap();
        b.place_pending(U, BTC, PosSide::Short, 1000, 5_850_000, 2, 5, None, None)
            .unwrap();
        b.take_events(U);
        let marks = HashMap::from([(BTC, 5_800_000)]);
        if use_market {
            let gap = HashSet::new();
            b.check_market(
                &view(&marks, &marks, &gap, &never, false),
                true,
                &HashSet::new(),
            );
        } else {
            b.check_with_frozen(&marks, true, &HashSet::new());
        }
        (b.take_events(U), b.balance(U))
    };
    assert_eq!(run(true), run(false));
}
