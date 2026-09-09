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

/// Строка-цена/объём Binance → целые raw-единицы.
///
/// `None` — строку разобрать не удалось. Раньше здесь стоял `unwrap_or(0.0)`, и
/// неразобранная цена превращалась в **цену 0**, неотличимую от котировки: отказ
/// уезжал в тикеры, свечи и стакан под видом данных (BUG-011). Ноль — правдоподобное
/// значение, поэтому отказ обязан выходить наружу отказом.
fn to_raw(s: &str, dec: u8) -> Option<i64> {
    let v: f64 = s.parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    Some((v * 10f64.powi(dec as i32)).round() as i64)
}

/// Стакан (bids/asks) в raw-единицах.
#[derive(Clone, Default)]
pub struct Depth {
    pub bids: Vec<(i64, i64)>,
    pub asks: Vec<(i64, i64)>,
}

/// Запросить стакан (order book) с Binance REST.
pub async fn fetch_depth(binance: &str, limit: usize, pd: u8, qd: u8) -> Result<Depth, String> {
    let url = format!("{REST}/api/v3/depth?symbol={binance}&limit={limit}");
    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("binance {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    let (depth, skipped) = depth_from_json(&v, pd, qd);
    if skipped > 0 {
        eprintln!("[feed] {binance}: пропущено уровней стакана с неразобранными числами: {skipped}");
    }
    Ok(depth)
}

/// Разобрать стакан из ответа Binance. Возвращает `(стакан, сколько уровней пропущено)`.
///
/// Уровень с неразобранными числами пропускаем, но счётчик отдаём наружу: молчаливый
/// пропуск искажает стакан, а знать об этом надо (BUG-011).
fn depth_from_json(v: &Value, pd: u8, qd: u8) -> (Depth, usize) {
    let mut skipped = 0usize;
    let mut parse = |side: &Value| -> Vec<(i64, i64)> {
        side.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|lvl| {
                        let row = (|| {
                            let p = lvl.get(0)?.as_str()?;
                            let q = lvl.get(1)?.as_str()?;
                            Some((to_raw(p, pd)?, to_raw(q, qd)?))
                        })();
                        if row.is_none() {
                            skipped += 1;
                        }
                        row
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let depth = Depth { bids: parse(&v["bids"]), asks: parse(&v["asks"]) };
    (depth, skipped)
}

/// Изменение цены за текущую свечу таймфрейма (в процентах). Для статистики актива.
pub async fn fetch_change(binance: &str, interval: &str) -> Result<f64, String> {
    let url = format!("{REST}/api/v3/klines?symbol={binance}&interval={interval}&limit=1");
    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("binance {}", resp.status()));
    }
    let rows: Vec<Vec<Value>> = resp.json().await.map_err(|e| e.to_string())?;
    change_from_row(rows.first().ok_or("empty")?)
}

/// Изменение цены внутри одной свечи Binance (в процентах).
///
/// Отказ разбора — это отказ, а не «изменение 0.00 %»: ноль процентов означает
/// «рынок стоял», и отличить его от «мы не смогли посчитать» вызывающий обязан.
fn change_from_row(row: &[Value]) -> Result<f64, String> {
    let num = |i: usize| -> Result<f64, String> {
        row.get(i)
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("нет поля {i}"))?
            .parse::<f64>()
            .map_err(|e| format!("поле {i}: {e}"))
    };
    let (open, close) = (num(1)?, num(4)?);
    if open == 0.0 {
        return Err("open = 0, изменение не определено".into());
    }
    Ok((close - open) / open * 100.0)
}

/// Запросить исторические свечи с Binance REST.
pub async fn fetch_klines(binance: &str, interval: &str, limit: usize, pd: u8, qd: u8) -> Result<Vec<Kline>, String> {
    let url = format!("{REST}/api/v3/klines?symbol={binance}&interval={interval}&limit={limit}");
    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("binance {}", resp.status()));
    }
    let rows: Vec<Vec<Value>> = resp.json().await.map_err(|e| e.to_string())?;
    klines_from_rows(&rows, pd, qd)
}

/// Разобрать свечи из ответа Binance.
///
/// Свеча с неразобранным полем — отказ на весь ответ. Раньше такое поле молча
/// становилось нулём, и по графику с нулевым low принимали решение (BUG-011).
fn klines_from_rows(rows: &[Vec<Value>], pd: u8, qd: u8) -> Result<Vec<Kline>, String> {
    let raw_at = |r: &[Value], i: usize, dec: u8| -> Result<i64, String> {
        let s = r.get(i).and_then(|v| v.as_str()).ok_or_else(|| format!("нет поля {i}"))?;
        to_raw(s, dec).ok_or_else(|| format!("поле {i}: не число ({s})"))
    };
    rows.iter()
        .map(|r| {
            Ok(Kline {
                time: r.first().and_then(|v| v.as_i64()).ok_or("нет времени свечи")? / 1000,
                open: raw_at(r, 1, pd)?,
                high: raw_at(r, 2, pd)?,
                low: raw_at(r, 3, pd)?,
                close: raw_at(r, 4, pd)?,
                volume: raw_at(r, 5, qd)?,
            })
        })
        .collect()
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
                            // Класс ошибки здесь терялся: `Err(_) => break` уводил в
                            // бесшумный реконнект каждые 3с, и обрыв фида выглядел как
                            // тишина. Диагностика, съеденная рабочим циклом, — это отказ,
                            // превращённый в задержку.
                            Ok(Message::Close(f)) => {
                                eprintln!("[feed] соединение закрыто биржей: {f:?}");
                                break;
                            }
                            Err(e) => {
                                eprintln!("[feed] обрыв соединения: {e}");
                                break;
                            }
                            _ => {}
                        }
                    }
                    eprintln!("[feed] поток сообщений завершился");
                }
                Err(e) => eprintln!("[feed] ошибка подключения: {e}"),
            }
            eprintln!("[feed] переподключение через 3с");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}

/// Разобрать конверт комбинированного потока Binance: `{"stream": ..., "data": {...}}`.
///
/// Обрезанное или чужое сообщение — отказ, а не пустой конверт: пустой `stream`
/// молча не совпал бы ни с одной веткой, и потеря выглядела бы как тишина.
fn parse_envelope(text: &str) -> Result<(String, Value), String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let stream = v["stream"].as_str().ok_or("нет поля stream")?.to_string();
    let data = v.get("data").ok_or("нет поля data")?.clone();
    Ok((stream, data))
}

/// Котировка из `@ticker`. `None` — хотя бы одно число не разобралось.
///
/// Пропускаем **всю** котировку, а не отдельное поле: прежний `unwrap_or("0")` клал
/// в тикер цену 0, а тикер — источник марк-цены, по которой считаются P&L, маржа и
/// триггеры SL/TP. Нулевой bid означал бы сделку по цене ноль (BUG-011).
fn ticker_from_data(data: &Value, pd: u8) -> Option<Ticker> {
    let raw = |k: &str| data[k].as_str().and_then(|s| to_raw(s, pd));
    Some(Ticker {
        last: raw("c")?,
        high: raw("h")?,
        bid: raw("b")?,
        ask: raw("a")?,
        change: data["P"].as_str()?.parse::<f64>().ok()?,
    })
}

/// Сделка из `@aggTrade` → `(цена, объём, сторона агрессора)`. `None` — не разобралось.
fn trade_from_data(data: &Value, pd: u8, qd: u8) -> Option<(i64, i64, &'static str)> {
    let price = data["p"].as_str().and_then(|s| to_raw(s, pd))?;
    let qty = data["q"].as_str().and_then(|s| to_raw(s, qd))?;
    // m = isBuyerMaker: покупатель — мейкер, значит агрессор — продавец.
    let side = if data["m"].as_bool().unwrap_or(false) { "sell" } else { "buy" };
    Some((price, qty, side))
}

async fn handle_msg(state: &AppState, index: &HashMap<String, (u32, u8, u8)>, text: &str) {
    let (stream, data) = match parse_envelope(text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[feed] нераспознанное сообщение ({e}): {text:.120}");
            return;
        }
    };
    let sym = data["s"].as_str().unwrap_or("");
    let Some(&(id, pd, qd)) = index.get(sym) else { return };

    if stream.ends_with("@ticker") {
        match ticker_from_data(&data, pd) {
            Some(t) => {
                state.tickers.lock().await.insert(id, t);
            }
            None => eprintln!("[feed] {sym}: котировка с неразобранными числами — пропущена"),
        }
    } else if stream.ends_with("@aggTrade") {
        match trade_from_data(&data, pd, qd) {
            Some((price, qty, side)) => {
                let msg = serde_json::json!([{
                    "type": "trade", "instrument": id, "price": price, "qty": qty, "taker_side": side
                }])
                .to_string();
                let _ = state.events_tx.send(msg);
            }
            None => eprintln!("[feed] {sym}: сделка с неразобранными числами — пропущена"),
        }
    }
}
