//! Демо ядра: прогоняет сценарий и печатает стакан «лестницей» + сделки.
//!
//! Запуск (через Docker-тулчейн, ADR-004):
//!   docker run --rm -e CARGO_TARGET_DIR=/tmp/target -v "<repo>:/app" -w /app \
//!     rust:slim cargo run --manifest-path core/Cargo.toml --example demo
//! Либо: `bash scripts/demo.sh`.

use exchange_core::{
    AssetId, Command, DepthSnapshot, Event, Instrument, InstrumentId, MatchingEngine, OrderId,
    OrderType, Price, Qty, Side, TimeInForce,
};

const INST: InstrumentId = InstrumentId(1);

fn place(e: &mut MatchingEngine, id: u64, side: Side, price: Option<i64>, qty: i64) -> Vec<Event> {
    let order_type = match price {
        Some(p) => OrderType::Limit { price: Price(p) },
        None => OrderType::Market,
    };
    e.apply(Command::PlaceOrder {
        instrument: INST,
        id: OrderId(id),
        side,
        order_type,
        qty: Qty(qty),
        tif: TimeInForce::Gtc,
    })
}

fn bar(qty: i64) -> String {
    "█".repeat((qty as usize).min(40))
}

fn print_book(snap: &DepthSnapshot) {
    println!("        price    qty  ord");
    // Аски сверху вниз: от высокой цены к лучшей (низкой).
    for lvl in snap.asks.iter().rev() {
        println!("  \x1b[31mASK\x1b[0m  {:>6} {:>6} {:>4}  {}", lvl.price.0, lvl.qty.0, lvl.orders, bar(lvl.qty.0));
    }
    println!("       ----------------------- spread");
    // Биды: лучший (высокий) сверху.
    for lvl in &snap.bids {
        println!("  \x1b[32mBID\x1b[0m  {:>6} {:>6} {:>4}  {}", lvl.price.0, lvl.qty.0, lvl.orders, bar(lvl.qty.0));
    }
}

fn print_trades(events: &[Event]) {
    for e in events {
        if let Event::Trade { price, qty, taker, maker, .. } = e {
            println!("  сделка: {} @ {}  (taker #{} × maker #{})", qty.0, price.0, taker.0, maker.0);
        }
    }
}

fn main() {
    let mut e = MatchingEngine::new();
    e.register_instrument(Instrument {
        id: INST,
        symbol: "BTC-USDT".to_string(),
        base: AssetId(1),
        quote: AssetId(2),
        price_decimals: 2,
        qty_decimals: 3,
        tick_size: Price(1),
        lot_size: Qty(1),
        min_qty: Qty(1),
    });

    println!("=== 1. Наполняем стакан (BTC-USDT) ===");
    place(&mut e, 1, Side::Sell, Some(103), 8);
    place(&mut e, 2, Side::Sell, Some(102), 5);
    place(&mut e, 3, Side::Sell, Some(101), 10);
    place(&mut e, 4, Side::Buy, Some(100), 7);
    place(&mut e, 5, Side::Buy, Some(99), 4);
    place(&mut e, 6, Side::Buy, Some(98), 6);
    print_book(&e.snapshot(INST, 10).unwrap());

    println!("\n=== 2. Рыночная покупка 12 (сметает лучшие аски) ===");
    let ev = place(&mut e, 7, Side::Buy, None, 12);
    print_trades(&ev);
    print_book(&e.snapshot(INST, 10).unwrap());

    println!("\n=== 3. Лимитная покупка 6 @ 100 (встаёт в стакан) ===");
    place(&mut e, 8, Side::Buy, Some(100), 6);
    print_book(&e.snapshot(INST, 10).unwrap());
}
