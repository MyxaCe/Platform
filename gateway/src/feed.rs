//! Реальные крипто-данные с Binance (ADR-013): REST-свечи + WS живые цены/сделки.
//!
//! Публичные эндпоинты Binance, без ключа. Строки-цены парсятся в целые raw-единицы по
//! `price_decimals`/`qty_decimals` каждой пары.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::AppState;

const REST: &str = "https://api.binance.com";
const WS: &str = "wss://stream.binance.com:9443";

/// Инструмент фида (крипто-пара Binance).
#[derive(Clone)]
pub struct FeedInstrument {
    pub id: u32,
    pub symbol: String,   // отображаемый, "ETH-USDT"
    pub binance: String,  // "ETHUSDT"
    pub base: u32,
    pub quote: u32,
    pub price_decimals: u8,
    pub qty_decimals: u8,
}

/// Живая котировка (raw-единицы).
#[derive(Clone, Copy, Default)]
pub struct Ticker {
    pub last: i64,
    pub change: f64,
    pub high: i64,
    pub bid: i64,
    pub ask: i64,
}

/// Свеча (raw-единицы).
#[derive(Clone, Copy)]
pub struct Kline {
    pub time: i64,
    pub open: i64,
    pub high: i64,
    pub low: i64,
    pub close: i64,
    pub volume: i64,
}

/// Топ-30 крипто-пар: (Binance-символ, price_decimals, qty_decimals).
const DEFS: &[(&str, u8, u8)] = &[
    ("BTCUSDT", 2, 5), ("ETHUSDT", 2, 4), ("BNBUSDT", 2, 3), ("SOLUSDT", 2, 3),
    ("XRPUSDT", 4, 1), ("DOGEUSDT", 5, 0), ("ADAUSDT", 4, 1), ("TRXUSDT", 5, 0),
    ("AVAXUSDT", 2, 2), ("LINKUSDT", 3, 2), ("DOTUSDT", 3, 2), ("LTCUSDT", 2, 3),
    ("BCHUSDT", 2, 3), ("UNIUSDT", 3, 2), ("ATOMUSDT", 3, 2), ("XLMUSDT", 5, 0),
    ("ETCUSDT", 3, 2), ("NEARUSDT", 3, 1), ("FILUSDT", 3, 2), ("APTUSDT", 3, 2),
    ("ARBUSDT", 4, 1), ("OPUSDT", 4, 1), ("INJUSDT", 3, 2), ("AAVEUSDT", 2, 3),
    ("HBARUSDT", 5, 0), ("VETUSDT", 5, 0), ("ICPUSDT", 3, 2), ("SHIBUSDT", 8, 0),
    ("SANDUSDT", 4, 0), ("MANAUSDT", 4, 0),
];

/// Список инструментов фида (id = индекс+1, base-актив = id, quote = 100/USDT).
pub fn instruments() -> Vec<FeedInstrument> {
    DEFS.iter()
        .enumerate()
        .map(|(i, &(bn, pd, qd))| FeedInstrument {
            id: i as u32 + 1,
            symbol: bn.replace("USDT", "-USDT"),
            binance: bn.to_string(),
            base: i as u32 + 1,
            quote: 100,
            price_decimals: pd,
            qty_decimals: qd,
        })
        .collect()
}

/// Таймфрейм (сек) → интервал Binance.
pub fn interval(tf: u32) -> &'static str {
    match tf {
        60 => "1m",
        300 => "5m",
        900 => "15m",
        1800 => "30m",
        3600 => "1h",
        14400 => "4h",
        86400 => "1d",
        604800 => "1w",
        _ => "1h",
    }
}

fn to_raw(s: &str, dec: u8) -> i64 {
    (s.parse::<f64>().unwrap_or(0.0) * 10f64.powi(dec as i32)).round() as i64
}

/// Запросить исторические свечи с Binance REST.
pub async fn fetch_klines(binance: &str, interval: &str, limit: usize, pd: u8, qd: u8) -> Result<Vec<Kline>, String> {
    let url = format!("{REST}/api/v3/klines?symbol={binance}&interval={interval}&limit={limit}");
    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("binance {}", resp.status()));
    }
    let rows: Vec<Vec<Value>> = resp.json().await.map_err(|e| e.to_string())?;
    let str_at = |r: &Vec<Value>, i: usize| r.get(i).and_then(|v| v.as_str()).unwrap_or("0").to_string();
    Ok(rows
        .iter()
        .map(|r| Kline {
            time: r.first().and_then(|v| v.as_i64()).unwrap_or(0) / 1000,
            open: to_raw(&str_at(r, 1), pd),
            high: to_raw(&str_at(r, 2), pd),
            low: to_raw(&str_at(r, 3), pd),
            close: to_raw(&str_at(r, 4), pd),
            volume: to_raw(&str_at(r, 5), qd),
        })
        .collect())
}

/// Запустить WS-фид Binance: @ticker (котировки) + @aggTrade (сделки). Реконнект при обрыве.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        // Индекс: Binance-символ (верхний регистр) → (id, pd, qd).
        let index: HashMap<String, (u32, u8, u8)> = state
            .feed_instruments
            .iter()
            .map(|f| (f.binance.clone(), (f.id, f.price_decimals, f.qty_decimals)))
            .collect();

        let streams: Vec<String> = state
            .feed_instruments
            .iter()
            .flat_map(|f| {
                let s = f.binance.to_lowercase();
                [format!("{s}@ticker"), format!("{s}@aggTrade")]
            })
            .collect();
        let url = format!("{WS}/stream?streams={}", streams.join("/"));

        loop {
            match connect_async(&url).await {
                Ok((mut ws, _)) => {
                    println!("[feed] Binance подключён ({} пар)", state.feed_instruments.len());
                    while let Some(msg) = ws.next().await {
                        match msg {
                            Ok(Message::Text(t)) => handle_msg(&state, &index, &t).await,
                            Ok(Message::Ping(p)) => {
                                let _ = ws.send(Message::Pong(p)).await;
                            }
                            Ok(Message::Close(_)) | Err(_) => break,
                            _ => {}
                        }
                    }
                }
                Err(e) => eprintln!("[feed] ошибка подключения: {e}"),
            }
            eprintln!("[feed] переподключение через 3с");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}

async fn handle_msg(state: &AppState, index: &HashMap<String, (u32, u8, u8)>, text: &str) {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let stream = v["stream"].as_str().unwrap_or("");
    let data = &v["data"];
    let sym = data["s"].as_str().unwrap_or("");
    let Some(&(id, pd, qd)) = index.get(sym) else { return };

    if stream.ends_with("@ticker") {
        let t = Ticker {
            last: to_raw(data["c"].as_str().unwrap_or("0"), pd),
            change: data["P"].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            high: to_raw(data["h"].as_str().unwrap_or("0"), pd),
            bid: to_raw(data["b"].as_str().unwrap_or("0"), pd),
            ask: to_raw(data["a"].as_str().unwrap_or("0"), pd),
        };
        state.tickers.lock().await.insert(id, t);
    } else if stream.ends_with("@aggTrade") {
        let price = to_raw(data["p"].as_str().unwrap_or("0"), pd);
        let qty = to_raw(data["q"].as_str().unwrap_or("0"), qd);
        // m = isBuyerMaker: покупатель — мейкер, значит агрессор — продавец.
        let side = if data["m"].as_bool().unwrap_or(false) { "sell" } else { "buy" };
        let msg = serde_json::json!([{
            "type": "trade", "instrument": id, "price": price, "qty": qty, "taker_side": side
        }])
        .to_string();
        let _ = state.events_tx.send(msg);
    }
}
