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
#[derive(Clone, Copy, Debug)]
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

    // Отметка живости ставится здесь — на **принятом и опознанном** сообщении, а не
    // при открытии сокета: живость фида доказывают данные, а не факт подключения.
    state.mark_feed_alive();

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

// ============================ Тесты разбора ================================
//
// Проверяем поведение при **битом входе**, а не при здоровом: здоровый вход
// доказывает связность кода, а не то, что отказ остаётся отказом (BUG-011, TRM-07).
// Отказы Binance на живом фиде не воспроизводятся по желанию — поэтому мусор
// подаётся чистым функциям напрямую.
//
// Общее требование ко всем случаям ниже: **ни один битый вход не должен превратиться
// в число**. Допустимы только отказ (`Err`/`None`) и пропуск.
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- to_raw: источник всех остальных нулей ----------------------------

    #[test]
    fn to_raw_parses_valid_price() {
        assert_eq!(to_raw("123.45", 2), Some(12345));
        assert_eq!(to_raw("0.00000413", 8), Some(413));
    }

    /// Не число → отказ. Раньше здесь стоял `unwrap_or(0.0)` и получалась цена 0.
    #[test]
    fn to_raw_rejects_garbage_instead_of_zero() {
        for bad in ["", "abc", "1.2.3", "--5", "1,5", " "] {
            assert_eq!(to_raw(bad, 2), None, "строка {bad:?} не должна становиться числом");
        }
    }

    /// NaN и бесконечность разбираются как f64 — и обязаны быть отвергнуты:
    /// `NaN as i64` даёт 0, то есть снова цену 0.
    #[test]
    fn to_raw_rejects_non_finite() {
        assert_eq!(to_raw("NaN", 2), None);
        assert_eq!(to_raw("inf", 2), None);
        assert_eq!(to_raw("-inf", 2), None);
    }

    // ---- Свечи: битое поле роняет ответ, а не рисует нулевой low -----------

    fn kline_row() -> Vec<Value> {
        json!([1_700_000_000_000i64, "100.00", "110.00", "90.00", "105.00", "12.5"])
            .as_array()
            .unwrap()
            .clone()
    }

    #[test]
    fn klines_parse_valid_row() {
        let out = klines_from_rows(&[kline_row()], 2, 2).expect("здоровая строка разбирается");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].time, 1_700_000_000);
        assert_eq!((out[0].open, out[0].high, out[0].low, out[0].close), (10000, 11000, 9000, 10500));
    }

    #[test]
    fn klines_reject_unparsable_field() {
        let mut row = kline_row();
        row[3] = json!("не число"); // low
        let err = klines_from_rows(&[row], 2, 2).expect_err("битое поле обязано ронять ответ");
        assert!(err.contains("поле 3"), "ошибка должна называть поле, а не молчать: {err}");
    }

    /// Поле не того типа (число вместо строки Binance) — тоже отказ.
    #[test]
    fn klines_reject_wrong_type() {
        let mut row = kline_row();
        row[1] = json!(100.0);
        assert!(klines_from_rows(&[row], 2, 2).is_err());
    }

    #[test]
    fn klines_reject_null_field() {
        let mut row = kline_row();
        row[4] = Value::Null;
        assert!(klines_from_rows(&[row], 2, 2).is_err());
    }

    /// Обрезанная строка (полей меньше, чем нужно) — отказ, а не свеча с нулями.
    #[test]
    fn klines_reject_truncated_row() {
        let row: Vec<Value> = kline_row().into_iter().take(3).collect();
        assert!(klines_from_rows(&[row], 2, 2).is_err());
    }

    /// Одна битая свеча роняет весь ответ: половина графика хуже отсутствующего.
    #[test]
    fn klines_one_bad_row_fails_whole_response() {
        let mut bad = kline_row();
        bad[2] = json!("x");
        assert!(klines_from_rows(&[kline_row(), bad, kline_row()], 2, 2).is_err());
    }

    // ---- Стакан: битый уровень пропускается и считается --------------------

    #[test]
    fn depth_parses_and_counts_skipped() {
        let v = json!({
            "bids": [["100.00", "1.000"], ["мусор", "1.000"], ["99.00", "2.000"]],
            "asks": [["101.00", "1.000"], ["102.00", null]],
        });
        let (d, skipped) = depth_from_json(&v, 2, 3);
        assert_eq!(d.bids, vec![(10000, 1000), (9900, 2000)]);
        assert_eq!(d.asks, vec![(10100, 1000)]);
        assert_eq!(skipped, 2, "пропуски обязаны быть посчитаны, иначе они молчаливые");
    }

    #[test]
    fn depth_missing_sides_is_empty_not_panic() {
        let (d, skipped) = depth_from_json(&json!({}), 2, 3);
        assert!(d.bids.is_empty() && d.asks.is_empty());
        assert_eq!(skipped, 0);
    }

    // ---- Изменение цены: отказ вместо «0.00 %» -----------------------------

    #[test]
    fn change_computed_from_valid_row() {
        let row = kline_row();
        let ch = change_from_row(&row).expect("здоровая строка считается");
        assert!((ch - 5.0).abs() < 1e-9, "ожидали +5%, получили {ch}");
    }

    /// Ключевой случай: отказ не должен выглядеть как «рынок стоял».
    #[test]
    fn change_rejects_garbage_instead_of_zero_percent() {
        let mut row = kline_row();
        row[4] = json!("не число");
        assert!(change_from_row(&row).is_err());

        let mut row = kline_row();
        row[1] = Value::Null;
        assert!(change_from_row(&row).is_err());

        assert!(change_from_row(&[]).is_err());
    }

    #[test]
    fn change_rejects_zero_open() {
        let mut row = kline_row();
        row[1] = json!("0");
        assert!(change_from_row(&row).is_err(), "деление на ноль не должно давать 0%");
    }

    // ---- Котировка: пропускается целиком, а не собирается наполовину -------

    fn ticker_data() -> Value {
        json!({"s": "BTCUSDT", "c": "100.00", "P": "1.5", "h": "110.00", "b": "99.99", "a": "100.01"})
    }

    #[test]
    fn ticker_parses_valid() {
        let t = ticker_from_data(&ticker_data(), 2).expect("здоровая котировка разбирается");
        assert_eq!((t.last, t.high, t.bid, t.ask), (10000, 11000, 9999, 10001));
        assert!((t.change - 1.5).abs() < 1e-9);
    }

    /// Битое поле → котировки нет вовсе. Именно «нет», а не Ticker с нулём в этом
    /// поле: нулевой bid — это сделка по цене ноль.
    #[test]
    fn ticker_skipped_entirely_on_any_bad_field() {
        for key in ["c", "P", "h", "b", "a"] {
            let mut d = ticker_data();
            d[key] = json!("не число");
            assert!(ticker_from_data(&d, 2).is_none(), "поле {key}: котировка обязана быть пропущена целиком");

            let mut d = ticker_data();
            d[key] = Value::Null;
            assert!(ticker_from_data(&d, 2).is_none(), "поле {key} = null: котировка обязана быть пропущена");

            let mut d = ticker_data();
            d.as_object_mut().unwrap().remove(key);
            assert!(ticker_from_data(&d, 2).is_none(), "поле {key} отсутствует: котировка обязана быть пропущена");
        }
    }

    /// Поле не того типа: число вместо строки — Binance так не присылает, но если
    /// пришлёт, это не повод молча взять ноль.
    #[test]
    fn ticker_rejects_wrong_type() {
        let mut d = ticker_data();
        d["b"] = json!(99.99);
        assert!(ticker_from_data(&d, 2).is_none());
    }

    // ---- Сделка ------------------------------------------------------------

    #[test]
    fn trade_parses_valid_and_reads_side() {
        let d = json!({"p": "100.00", "q": "0.500", "m": true});
        assert_eq!(trade_from_data(&d, 2, 3), Some((10000, 500, "sell")));
        let d = json!({"p": "100.00", "q": "0.500", "m": false});
        assert_eq!(trade_from_data(&d, 2, 3), Some((10000, 500, "buy")));
    }

    #[test]
    fn trade_skipped_on_bad_numbers() {
        assert!(trade_from_data(&json!({"p": "x", "q": "0.5", "m": false}), 2, 3).is_none());
        assert!(trade_from_data(&json!({"p": "1.0", "q": null, "m": false}), 2, 3).is_none());
        assert!(trade_from_data(&json!({"m": false}), 2, 3).is_none());
    }

    // ---- Конверт потока ----------------------------------------------------

    #[test]
    fn envelope_parses_valid() {
        let (stream, data) = parse_envelope(r#"{"stream":"btcusdt@ticker","data":{"s":"BTCUSDT"}}"#)
            .expect("здоровый конверт разбирается");
        assert_eq!(stream, "btcusdt@ticker");
        assert_eq!(data["s"], json!("BTCUSDT"));
    }

    /// Обрезанное сообщение (WS-фрейм пришёл не целиком) — отказ с причиной.
    #[test]
    fn envelope_rejects_truncated() {
        assert!(parse_envelope(r#"{"stream":"btcusdt@ticker","data":{"s":"BTC"#).is_err());
        assert!(parse_envelope("").is_err());
        assert!(parse_envelope("не json вовсе").is_err());
    }

    /// Конверт без обязательных полей — тоже отказ: пустой `stream` не совпал бы ни
    /// с одной веткой, и сообщение потерялось бы молча.
    #[test]
    fn envelope_rejects_missing_fields() {
        assert!(parse_envelope(r#"{"data":{"s":"BTCUSDT"}}"#).is_err());
        assert!(parse_envelope(r#"{"stream":"btcusdt@ticker"}"#).is_err());
        assert!(parse_envelope(r#"{"stream":123,"data":{}}"#).is_err());
    }
}
