//! Тесты агрегации свечей.

use marketdata::{Candle, CandleStore};

#[test]
fn trades_in_same_bucket_aggregate() {
    let mut cs = CandleStore::new(vec![60], 100);
    // Все метки в корзине [960, 1019] → одна минутная свеча.
    cs.ingest(1, 960, 100, 5);
    cs.ingest(1, 990, 110, 3);
    cs.ingest(1, 1015, 90, 2);
    let c = cs.candles(1, 60, 10);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0], Candle { time: 960, open: 100, high: 110, low: 90, close: 90, volume: 10 });
}

#[test]
fn new_bucket_starts_new_candle() {
    let mut cs = CandleStore::new(vec![60], 100);
    cs.ingest(1, 1000, 100, 1); // bucket 960
    cs.ingest(1, 1100, 105, 1); // bucket 1080
    let c = cs.candles(1, 60, 10);
    assert_eq!(c.len(), 2);
    assert_eq!(c[0].time, 960);
    assert_eq!(c[1].time, 1080);
    assert_eq!(c[1].open, 105);
}

#[test]
fn multiple_timeframes_tracked_independently() {
    let mut cs = CandleStore::new(vec![60, 300], 100);
    cs.ingest(1, 900, 100, 1); // 1m bucket 900, 5m bucket 900
    cs.ingest(1, 960, 110, 1); // 1m bucket 960, тот же 5m bucket 900
    assert_eq!(cs.candles(1, 60, 10).len(), 2); // два минутных
    assert_eq!(cs.candles(1, 300, 10).len(), 1); // одна пятиминутная
    assert_eq!(cs.candles(1, 300, 10)[0].close, 110);
}

#[test]
fn candles_limit_returns_tail() {
    let mut cs = CandleStore::new(vec![60], 100);
    for i in 0..10 {
        cs.ingest(1, 960 + i * 60, 100 + i, 1);
    }
    let c = cs.candles(1, 60, 3);
    assert_eq!(c.len(), 3);
    assert_eq!(c[2].close, 109);
}

#[test]
fn seed_replaces_series() {
    let mut cs = CandleStore::new(vec![60], 100);
    cs.seed(1, 60, vec![Candle { time: 0, open: 1, high: 2, low: 1, close: 2, volume: 1 }]);
    assert_eq!(cs.last_price(1, 60), Some(2));
}
