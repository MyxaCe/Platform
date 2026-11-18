use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::AppState;

const REST: &str = "https://api.binance.com";
const WS: &str = "wss://stream.binance.com:9443";

#[derive(Clone)]
pub struct FeedInstrument {
    pub id: u32,
    pub symbol: String,
    pub binance: String,
    pub base: u32,
    pub quote: u32,
    pub price_decimals: u8,
    pub qty_decimals: u8,
}

#[derive(Clone, Copy, Default)]
pub struct Ticker {
    pub last: i64,
    pub change: f64,
    pub high: i64,
    pub bid: i64,
    pub ask: i64,
    pub low: Option<i64>,
    pub volume: Option<i64>,
    pub quote_volume: Option<i64>,
}

#[derive(Clone, Copy, Debug)]
pub struct Kline {
    pub time: i64,
    pub open: i64,
    pub high: i64,
    pub low: i64,
    pub close: i64,
    pub volume: i64,
}

const DEFS: &[(&str, u8, u8)] = &[
    ("BTCUSDT", 2, 5),
    ("ETHUSDT", 2, 4),
    ("BNBUSDT", 2, 3),
    ("SOLUSDT", 2, 3),
    ("XRPUSDT", 4, 1),
    ("DOGEUSDT", 5, 0),
    ("ADAUSDT", 4, 1),
    ("TRXUSDT", 5, 0),
    ("AVAXUSDT", 2, 2),
    ("LINKUSDT", 3, 2),
    ("DOTUSDT", 3, 2),
    ("LTCUSDT", 2, 3),
    ("BCHUSDT", 2, 3),
    ("UNIUSDT", 3, 2),
    ("ATOMUSDT", 3, 2),
    ("XLMUSDT", 5, 0),
    ("ETCUSDT", 3, 2),
    ("NEARUSDT", 3, 1),
    ("FILUSDT", 3, 2),
    ("APTUSDT", 3, 2),
    ("ARBUSDT", 4, 1),
    ("OPUSDT", 4, 1),
    ("INJUSDT", 3, 2),
    ("AAVEUSDT", 2, 3),
    ("HBARUSDT", 5, 0),
    ("VETUSDT", 5, 0),
    ("ICPUSDT", 3, 2),
    ("SHIBUSDT", 8, 0),
    ("SANDUSDT", 4, 0),
    ("MANAUSDT", 4, 0),
];

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

fn to_raw(s: &str, dec: u8) -> Option<i64> {
    let v: f64 = s.parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    Some((v * 10f64.powi(dec as i32)).round() as i64)
}

#[derive(Clone, Default)]
pub struct Depth {
    pub bids: Vec<(i64, i64)>,
    pub asks: Vec<(i64, i64)>,
}

pub async fn fetch_depth(binance: &str, limit: usize, pd: u8, qd: u8) -> Result<Depth, String> {
    let url = format!("{REST}/api/v3/depth?symbol={binance}&limit={limit}");
    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("binance {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    let (depth, skipped) = depth_from_json(&v, pd, qd);
    if skipped > 0 {
        eprintln!(
            "[feed] {binance}: пропущено уровней стакана с неразобранными числами: {skipped}"
        );
    }
    Ok(depth)
}

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
    let depth = Depth {
        bids: parse(&v["bids"]),
        asks: parse(&v["asks"]),
    };
    (depth, skipped)
}

pub async fn fetch_change(binance: &str, interval: &str) -> Result<f64, String> {
    let url = format!("{REST}/api/v3/klines?symbol={binance}&interval={interval}&limit=1");
    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("binance {}", resp.status()));
    }
    let rows: Vec<Vec<Value>> = resp.json().await.map_err(|e| e.to_string())?;
    change_from_row(rows.first().ok_or("empty")?)
}

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

pub async fn fetch_klines(
    binance: &str,
    interval: &str,
    limit: usize,
    pd: u8,
    qd: u8,
) -> Result<Vec<Kline>, String> {
    let url = format!("{REST}/api/v3/klines?symbol={binance}&interval={interval}&limit={limit}");
    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("binance {}", resp.status()));
    }
    let rows: Vec<Vec<Value>> = resp.json().await.map_err(|e| e.to_string())?;
    klines_from_rows(&rows, pd, qd)
}

fn klines_from_rows(rows: &[Vec<Value>], pd: u8, qd: u8) -> Result<Vec<Kline>, String> {
    let raw_at = |r: &[Value], i: usize, dec: u8| -> Result<i64, String> {
        let s = r
            .get(i)
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("нет поля {i}"))?;
        to_raw(s, dec).ok_or_else(|| format!("поле {i}: не число ({s})"))
    };
    rows.iter()
        .map(|r| {
            Ok(Kline {
                time: r
                    .first()
                    .and_then(|v| v.as_i64())
                    .ok_or("нет времени свечи")?
                    / 1000,
                open: raw_at(r, 1, pd)?,
                high: raw_at(r, 2, pd)?,
                low: raw_at(r, 3, pd)?,
                close: raw_at(r, 4, pd)?,
                volume: raw_at(r, 5, qd)?,
            })
        })
        .collect()
}

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
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
                    println!(
                        "[feed] Binance подключён ({} пар)",
                        state.feed_instruments.len()
                    );
                    while let Some(msg) = ws.next().await {
                        match msg {
                            Ok(Message::Text(t)) => handle_msg(&state, &index, &t).await,
                            Ok(Message::Ping(p)) => {
                                let _ = ws.send(Message::Pong(p)).await;
                            }
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

fn parse_envelope(text: &str) -> Result<(String, Value), String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let stream = v["stream"].as_str().ok_or("нет поля stream")?.to_string();
    let data = v.get("data").ok_or("нет поля data")?.clone();
    Ok((stream, data))
}

fn ticker_from_data(data: &Value, pd: u8, qd: u8) -> Option<Ticker> {
    let raw = |k: &str| data[k].as_str().and_then(|s| to_raw(s, pd));
    Some(Ticker {
        last: raw("c")?,
        high: raw("h")?,
        bid: raw("b")?,
        ask: raw("a")?,
        change: data["P"].as_str()?.parse::<f64>().ok()?,
        low: raw("l"),
        volume: data["v"].as_str().and_then(|s| to_raw(s, qd)),
        quote_volume: raw("q"),
    })
}

fn trade_from_data(data: &Value, pd: u8, qd: u8) -> Option<(i64, i64, &'static str)> {
    let price = data["p"].as_str().and_then(|s| to_raw(s, pd))?;
    let qty = data["q"].as_str().and_then(|s| to_raw(s, qd))?;
    let side = if data["m"].as_bool().unwrap_or(false) {
        "sell"
    } else {
        "buy"
    };
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
    let Some(&(id, pd, qd)) = index.get(sym) else {
        return;
    };

    state.mark_feed_alive();

    if stream.ends_with("@ticker") {
        match ticker_from_data(&data, pd, qd) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn to_raw_parses_valid_price() {
        assert_eq!(to_raw("123.45", 2), Some(12345));
        assert_eq!(to_raw("0.00000413", 8), Some(413));
    }

    #[test]
    fn to_raw_rejects_garbage_instead_of_zero() {
        for bad in ["", "abc", "1.2.3", "--5", "1,5", " "] {
            assert_eq!(
                to_raw(bad, 2),
                None,
                "строка {bad:?} не должна становиться числом"
            );
        }
    }

    #[test]
    fn to_raw_rejects_non_finite() {
        assert_eq!(to_raw("NaN", 2), None);
        assert_eq!(to_raw("inf", 2), None);
        assert_eq!(to_raw("-inf", 2), None);
    }

    fn kline_row() -> Vec<Value> {
        json!([
            1_700_000_000_000i64,
            "100.00",
            "110.00",
            "90.00",
            "105.00",
            "12.5"
        ])
        .as_array()
        .unwrap()
        .clone()
    }

    #[test]
    fn klines_parse_valid_row() {
        let out = klines_from_rows(&[kline_row()], 2, 2).expect("здоровая строка разбирается");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].time, 1_700_000_000);
        assert_eq!(
            (out[0].open, out[0].high, out[0].low, out[0].close),
            (10000, 11000, 9000, 10500)
        );
    }

    #[test]
    fn klines_reject_unparsable_field() {
        let mut row = kline_row();
        row[3] = json!("не число");
        let err = klines_from_rows(&[row], 2, 2).expect_err("битое поле обязано ронять ответ");
        assert!(
            err.contains("поле 3"),
            "ошибка должна называть поле, а не молчать: {err}"
        );
    }

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

    #[test]
    fn klines_reject_truncated_row() {
        let row: Vec<Value> = kline_row().into_iter().take(3).collect();
        assert!(klines_from_rows(&[row], 2, 2).is_err());
    }

    #[test]
    fn klines_one_bad_row_fails_whole_response() {
        let mut bad = kline_row();
        bad[2] = json!("x");
        assert!(klines_from_rows(&[kline_row(), bad, kline_row()], 2, 2).is_err());
    }

    #[test]
    fn depth_parses_and_counts_skipped() {
        let v = json!({
            "bids": [["100.00", "1.000"], ["мусор", "1.000"], ["99.00", "2.000"]],
            "asks": [["101.00", "1.000"], ["102.00", null]],
        });
        let (d, skipped) = depth_from_json(&v, 2, 3);
        assert_eq!(d.bids, vec![(10000, 1000), (9900, 2000)]);
        assert_eq!(d.asks, vec![(10100, 1000)]);
        assert_eq!(
            skipped, 2,
            "пропуски обязаны быть посчитаны, иначе они молчаливые"
        );
    }

    #[test]
    fn depth_missing_sides_is_empty_not_panic() {
        let (d, skipped) = depth_from_json(&json!({}), 2, 3);
        assert!(d.bids.is_empty() && d.asks.is_empty());
        assert_eq!(skipped, 0);
    }

    #[test]
    fn change_computed_from_valid_row() {
        let row = kline_row();
        let ch = change_from_row(&row).expect("здоровая строка считается");
        assert!((ch - 5.0).abs() < 1e-9, "ожидали +5%, получили {ch}");
    }

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
        assert!(
            change_from_row(&row).is_err(),
            "деление на ноль не должно давать 0%"
        );
    }

    fn ticker_data() -> Value {
        json!({"s": "BTCUSDT", "c": "100.00", "P": "1.5", "h": "110.00", "b": "99.99", "a": "100.01"})
    }

    #[test]
    fn ticker_parses_valid() {
        let t = ticker_from_data(&ticker_data(), 2, 3).expect("здоровая котировка разбирается");
        assert_eq!((t.last, t.high, t.bid, t.ask), (10000, 11000, 9999, 10001));
        assert!((t.change - 1.5).abs() < 1e-9);
    }

    #[test]
    fn ticker_24h_fields_parse_and_do_not_veto_quote() {
        let mut d = ticker_data();
        d["l"] = json!("90.50");
        d["v"] = json!("1234.567");
        d["q"] = json!("123456.78");
        let t = ticker_from_data(&d, 2, 3).expect("котировка разбирается");
        assert_eq!(
            (t.low, t.volume, t.quote_volume),
            (Some(9050), Some(1_234_567), Some(12_345_678))
        );

        d["l"] = json!("не число");
        d["v"] = Value::Null;
        let t = ticker_from_data(&d, 2, 3).expect("битые поля шапки котировку не отменяют");
        assert_eq!(
            (t.low, t.volume),
            (None, None),
            "битое — прочерк, а не ноль"
        );
        assert_eq!(t.last, 10000);
    }

    #[test]
    fn ticker_skipped_entirely_on_any_bad_field() {
        for key in ["c", "P", "h", "b", "a"] {
            let mut d = ticker_data();
            d[key] = json!("не число");
            assert!(
                ticker_from_data(&d, 2, 3).is_none(),
                "поле {key}: котировка обязана быть пропущена целиком"
            );

            let mut d = ticker_data();
            d[key] = Value::Null;
            assert!(
                ticker_from_data(&d, 2, 3).is_none(),
                "поле {key} = null: котировка обязана быть пропущена"
            );

            let mut d = ticker_data();
            d.as_object_mut().unwrap().remove(key);
            assert!(
                ticker_from_data(&d, 2, 3).is_none(),
                "поле {key} отсутствует: котировка обязана быть пропущена"
            );
        }
    }

    #[test]
    fn ticker_rejects_wrong_type() {
        let mut d = ticker_data();
        d["b"] = json!(99.99);
        assert!(ticker_from_data(&d, 2, 3).is_none());
    }

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

    #[test]
    fn envelope_parses_valid() {
        let (stream, data) =
            parse_envelope(r#"{"stream":"btcusdt@ticker","data":{"s":"BTCUSDT"}}"#)
                .expect("здоровый конверт разбирается");
        assert_eq!(stream, "btcusdt@ticker");
        assert_eq!(data["s"], json!("BTCUSDT"));
    }

    #[test]
    fn envelope_rejects_truncated() {
        assert!(parse_envelope(r#"{"stream":"btcusdt@ticker","data":{"s":"BTC"#).is_err());
        assert!(parse_envelope("").is_err());
        assert!(parse_envelope("не json вовсе").is_err());
    }

    #[test]
    fn envelope_rejects_missing_fields() {
        assert!(parse_envelope(r#"{"data":{"s":"BTCUSDT"}}"#).is_err());
        assert!(parse_envelope(r#"{"stream":"btcusdt@ticker"}"#).is_err());
        assert!(parse_envelope(r#"{"stream":123,"data":{}}"#).is_err());
    }
}
