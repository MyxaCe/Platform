//! # marketdata
//!
//! Агрегация сделок в свечи (OHLCV) по таймфреймам (ADR-012). Чистая логика без async и внешних
//! зависимостей — время передаётся снаружи (боундари-сервис). Цены/объёмы — целые (raw-единицы).

use std::collections::HashMap;

/// Свеча (OHLCV). `time` — начало корзины (unix-секунды). Цены — в raw-единицах инструмента.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candle {
    pub time: i64,
    pub open: i64,
    pub high: i64,
    pub low: i64,
    pub close: i64,
    pub volume: i64,
}

/// Хранилище свечей по (инструмент, таймфрейм). Таймфреймы — в секундах.
#[derive(Debug)]
pub struct CandleStore {
    tfs: Vec<u32>,
    cap: usize,
    series: HashMap<(u32, u32), Vec<Candle>>,
}

impl CandleStore {
    pub fn new(tfs: Vec<u32>, cap: usize) -> Self {
        Self { tfs, cap, series: HashMap::new() }
    }

    pub fn timeframes(&self) -> &[u32] {
        &self.tfs
    }

    /// Начало корзины для метки времени и таймфрейма.
    pub fn bucket(ts: i64, tf: u32) -> i64 {
        ts - ts.rem_euclid(tf as i64)
    }

    /// Учесть сделку во всех таймфреймах.
    pub fn ingest(&mut self, instrument: u32, ts: i64, price: i64, qty: i64) {
        let (tfs, cap) = (self.tfs.clone(), self.cap);
        for tf in tfs {
            let b = Self::bucket(ts, tf);
            let s = self.series.entry((instrument, tf)).or_default();
            match s.last_mut() {
                Some(c) if c.time == b => {
                    if price > c.high {
                        c.high = price;
                    }
                    if price < c.low {
                        c.low = price;
                    }
                    c.close = price;
                    c.volume += qty;
                }
                Some(c) if b < c.time => { /* сделка «из прошлого» — игнорируем */ }
                _ => {
                    s.push(Candle { time: b, open: price, high: price, low: price, close: price, volume: qty });
                    if s.len() > cap {
                        s.remove(0);
                    }
                }
            }
        }
    }

    /// Заменить серию (для синтетического бэкфилла истории при старте).
    pub fn seed(&mut self, instrument: u32, tf: u32, candles: Vec<Candle>) {
        self.series.insert((instrument, tf), candles);
    }

    /// Последние `limit` свечей серии.
    pub fn candles(&self, instrument: u32, tf: u32, limit: usize) -> Vec<Candle> {
        match self.series.get(&(instrument, tf)) {
            Some(s) => s[s.len().saturating_sub(limit)..].to_vec(),
            None => Vec::new(),
        }
    }

    /// Цена последней сделки (close последней свечи) на таймфрейме.
    pub fn last_price(&self, instrument: u32, tf: u32) -> Option<i64> {
        self.series.get(&(instrument, tf)).and_then(|s| s.last()).map(|c| c.close)
    }
}
