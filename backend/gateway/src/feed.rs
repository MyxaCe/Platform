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

const DEFAULT_REST: &str = "https://api.binance.com";
const DEFAULT_WS: &str = "wss://stream.binance.com:9443";

fn base_from(value: Option<&str>, default: &str) -> String {
    match value.map(str::trim) {
        Some(v) if !v.is_empty() => v.trim_end_matches('/').to_string(),
        _ => default.to_string(),
    }
}

pub fn rest_base() -> &'static str {
    static BASE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BASE.get_or_init(|| {
        base_from(
            std::env::var("BINANCE_API_URL").ok().as_deref(),
            DEFAULT_REST,
        )
    })
}

pub fn ws_base() -> &'static str {
    static BASE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BASE.get_or_init(|| base_from(std::env::var("BINANCE_WS_URL").ok().as_deref(), DEFAULT_WS))
}

pub fn masked(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    match rest.split_once('/') {
        Some((host, _)) => format!("{scheme}://{host}/…"),
        None => url.to_string(),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    #[default]
    V1,
    V2,
}

impl Source {
    pub fn tag(self) -> &'static str {
        match self {
            Source::V1 => "v1",
            Source::V2 => "v2",
        }
    }

    pub fn parse(t: &str) -> Option<Source> {
        match t {
            "v1" => Some(Source::V1),
            "v2" => Some(Source::V2),
            _ => None,
        }
    }
}

/// Инструмент фида (крипто-пара Binance).
#[derive(Clone, Debug, Default)]
pub struct FeedInstrument {
    pub id: u32,
    pub symbol: String,  // отображаемый, "ETH-USDT"
    pub binance: String, // "ETHUSDT"
    pub base: u32,
    pub quote: u32,
    pub price_decimals: u8,
    pub qty_decimals: u8,
    pub canonical: String,
    pub source: Source,
    pub category: String,
    pub name: Option<String>,
    pub tick_size: Option<i64>,
    pub qty_step: Option<i64>,
    pub min_qty: Option<i64>,
    pub contract_size: Option<String>,
    pub lot_decimals: Option<u8>,
    pub has_depth: bool,
    pub has_trades: bool,
    pub has_1s: bool,
    pub session: Option<Value>,
    pub legacy: bool,
}

/// Живая котировка (raw-единицы).
#[derive(Clone, Copy, Default)]
pub struct Ticker {
    pub last: i64,
    pub change: f64,
    pub high: i64,
    pub bid: i64,
    pub ask: i64,
    /// 24ч-минимум (raw цены). Поля шапки инструмента, а не цены исполнения:
    /// поэтому `Option` и разбираются **отдельно** от котировки — битый объём
    /// не должен отнимать у счёта марк-цену, а битая марк-цена и так отвергает
    /// котировку целиком (см. `ticker_from_data`).
    pub low: Option<i64>,
    /// 24ч-объём в базовой валюте (raw объёма, `qty_decimals`).
    pub volume: Option<i64>,
    /// 24ч-оборот в котируемой валюте (raw цены, `price_decimals`).
    pub quote_volume: Option<i64>,
}

/// Свеча (raw-единицы).
#[derive(Clone, Copy, Debug)]
pub struct Kline {
    pub time: i64,
    pub open: i64,
    pub high: i64,
    pub low: i64,
    pub close: i64,
    pub volume: Option<i64>,
}

/// Топ-30 крипто-пар: (Binance-символ, price_decimals, qty_decimals).
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
            canonical: canonical_of_usdt(bn),
            source: Source::V1,
            category: "crypto".into(),
            has_depth: true,
            has_trades: true,
            has_1s: true,
            legacy: true,
            ..FeedInstrument::default()
        })
        .collect()
}

pub fn canonical_of_usdt(binance: &str) -> String {
    match binance.strip_suffix("USDT") {
        Some(base) if !base.is_empty() => format!("{base}USD"),
        _ => binance.to_string(),
    }
}

pub fn legacy_rows() -> Vec<persistence::InstrumentRow> {
    instruments()
        .into_iter()
        .map(|f| persistence::InstrumentRow {
            id: f.id,
            symbol: f.canonical,
            source: Source::V1.tag().into(),
            provider_symbol: f.binance,
            price_decimals: f.price_decimals,
            qty_decimals: f.qty_decimals,
        })
        .collect()
}

/// Таймфрейм (сек) → интервал Binance.
pub fn interval(tf: u32) -> &'static str {
    match tf {
        1 => "1s",
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
pub(crate) fn to_raw(s: &str, dec: u8) -> Option<i64> {
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
    let url = format!(
        "{}/api/v3/depth?symbol={binance}&limit={limit}",
        rest_base()
    );
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

/// Разобрать стакан из ответа Binance. Возвращает `(стакан, сколько уровней пропущено)`.
///
/// Уровень с неразобранными числами пропускаем, но счётчик отдаём наружу: молчаливый
/// пропуск искажает стакан, а знать об этом надо (BUG-011).
pub(crate) fn depth_from_json(v: &Value, pd: u8, qd: u8) -> (Depth, usize) {
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

/// Изменение цены за текущую свечу таймфрейма (в процентах). Для статистики актива.
pub async fn fetch_change(binance: &str, interval: &str) -> Result<f64, String> {
    let url = format!(
        "{}/api/v3/klines?symbol={binance}&interval={interval}&limit=1",
        rest_base()
    );
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
pub(crate) fn change_from_row(row: &[Value]) -> Result<f64, String> {
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
pub async fn fetch_klines(
    binance: &str,
    interval: &str,
    limit: usize,
    end: Option<i64>,
    pd: u8,
    qd: u8,
) -> Result<Vec<Kline>, String> {
    let mut url = format!(
        "{}/api/v3/klines?symbol={binance}&interval={interval}&limit={limit}",
        rest_base()
    );
    if let Some(end) = end {
        url.push_str(&format!("&endTime={end}"));
    }
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
                volume: Some(raw_at(r, 5, qd)?),
            })
        })
        .collect()
}

pub const RESUBSCRIBE_MIN_SECS: u64 = 30;

type V1Index = HashMap<String, (u32, u8, u8)>;

fn v1_index(state: &AppState) -> V1Index {
    state
        .catalog_snapshot()
        .into_iter()
        .filter(|f| f.source == Source::V1)
        .map(|f| (f.binance.clone(), (f.id, f.price_decimals, f.qty_decimals)))
        .collect()
}

pub fn stream_url(base: &str, symbols: &std::collections::BTreeSet<String>) -> String {
    let streams: Vec<String> = symbols
        .iter()
        .flat_map(|b| {
            let s = b.to_lowercase();
            [format!("{s}@ticker"), format!("{s}@aggTrade")]
        })
        .collect();
    format!("{base}/stream?streams={}", streams.join("/"))
}

/// Запустить WS-фид Binance: @ticker (котировки) + @aggTrade (сделки). Реконнект при обрыве.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        println!(
            "[feed] источник рыночных данных: REST {}, WS {}",
            masked(rest_base()),
            masked(ws_base())
        );
        let mut subs = state.subscriptions_v1();
        loop {
            let wanted = subs.borrow_and_update().clone();
            if wanted.is_empty() {
                if subs.changed().await.is_err() {
                    return;
                }
                continue;
            }
            let url = stream_url(ws_base(), &wanted);
            let mut ws = match connect_async(&url).await {
                Ok((ws, _)) => ws,
                Err(e) => {
                    eprintln!("[feed] ошибка подключения: {e}");
                    eprintln!("[feed] переподключение через 3с");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
            };
            println!("[feed] Binance подключён ({} пар)", wanted.len());
            let mut index = v1_index(&state);
            let mut current = wanted;
            let mut connected_at = tokio::time::Instant::now();
            let mut recheck = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    msg = ws.next() => match msg {
                        Some(Ok(Message::Text(t))) => handle_msg(&state, &index, &t).await,
                        Some(Ok(Message::Ping(p))) => {
                            let _ = ws.send(Message::Pong(p)).await;
                        }
                        Some(Ok(Message::Close(f))) => {
                            eprintln!("[feed] соединение закрыто биржей: {f:?}");
                            break;
                        }
                        Some(Err(e)) => {
                            eprintln!("[feed] обрыв соединения: {e}");
                            break;
                        }
                        None => break,
                        _ => {}
                    },
                    _ = recheck.tick() => {
                        let next = subs.borrow().clone();
                        if next == current || next.is_empty() {
                            continue;
                        }
                        if connected_at.elapsed() < Duration::from_secs(RESUBSCRIBE_MIN_SECS) {
                            continue;
                        }
                        match connect_async(&stream_url(ws_base(), &next)).await {
                            Ok((fresh, _)) => {
                                println!(
                                    "[feed] список пар изменился ({} → {}): новое соединение открыто до закрытия старого",
                                    current.len(),
                                    next.len()
                                );
                                let mut old = std::mem::replace(&mut ws, fresh);
                                let _ = old.close(None).await;
                                index = v1_index(&state);
                                current = next;
                                connected_at = tokio::time::Instant::now();
                            }
                            Err(e) => eprintln!("[feed] переподписка не удалась ({e}), работаем по прежнему списку"),
                        }
                    }
                }
            }
            eprintln!("[feed] поток сообщений завершился");
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

/// Сделка из `@aggTrade` → `(цена, объём, сторона агрессора)`. `None` — не разобралось.
fn trade_from_data(data: &Value, pd: u8, qd: u8) -> Option<(i64, i64, &'static str)> {
    let price = data["p"].as_str().and_then(|s| to_raw(s, pd))?;
    let qty = data["q"].as_str().and_then(|s| to_raw(s, qd))?;
    // m = isBuyerMaker: покупатель — мейкер, значит агрессор — продавец.
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

    // Отметка живости ставится здесь — на **принятом и опознанном** сообщении, а не
    // при открытии сокета: живость фида доказывают данные, а не факт подключения.
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
                    "type": "trade", "instrument": id, "price": price, "qty": qty, "taker_side": side, "time": data["T"].as_i64()
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

    #[test]
    fn one_second_timeframe_maps_to_1s() {
        assert_eq!(interval(1), "1s");
        assert_eq!(interval(60), "1m");
    }

    #[test]
    fn source_url_defaults_to_binance() {
        assert_eq!(base_from(None, DEFAULT_REST), "https://api.binance.com");
        assert_eq!(
            base_from(Some("  "), DEFAULT_WS),
            "wss://stream.binance.com:9443"
        );
    }

    #[test]
    fn source_url_keeps_path_and_drops_trailing_slash() {
        assert_eq!(
            base_from(Some("https://md.example/abc123/"), DEFAULT_REST),
            "https://md.example/abc123"
        );
    }

    #[test]
    fn masked_url_hides_path_key() {
        assert_eq!(masked("wss://md.example/abc123"), "wss://md.example/…");
        assert_eq!(masked("https://api.binance.com"), "https://api.binance.com");
    }

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
            assert_eq!(
                to_raw(bad, 2),
                None,
                "строка {bad:?} не должна становиться числом"
            );
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
        row[3] = json!("не число"); // low
        let err = klines_from_rows(&[row], 2, 2).expect_err("битое поле обязано ронять ответ");
        assert!(
            err.contains("поле 3"),
            "ошибка должна называть поле, а не молчать: {err}"
        );
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
        assert!(
            change_from_row(&row).is_err(),
            "деление на ноль не должно давать 0%"
        );
    }

    // ---- Котировка: пропускается целиком, а не собирается наполовину -------

    fn ticker_data() -> Value {
        json!({"s": "BTCUSDT", "c": "100.00", "P": "1.5", "h": "110.00", "b": "99.99", "a": "100.01"})
    }

    #[test]
    fn ticker_parses_valid() {
        let t = ticker_from_data(&ticker_data(), 2, 3).expect("здоровая котировка разбирается");
        assert_eq!((t.last, t.high, t.bid, t.ask), (10000, 11000, 9999, 10001));
        assert!((t.change - 1.5).abs() < 1e-9);
    }

    /// Поля шапки инструмента (24ч мин/объём/оборот) разбираются, и их порча
    /// **не отнимает котировку**: марк-цена важнее подписи в шапке.
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

    /// Битое поле → котировки нет вовсе. Именно «нет», а не Ticker с нулём в этом
    /// поле: нулевой bid — это сделка по цене ноль.
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

    /// Поле не того типа: число вместо строки — Binance так не присылает, но если
    /// пришлёт, это не повод молча взять ноль.
    #[test]
    fn ticker_rejects_wrong_type() {
        let mut d = ticker_data();
        d["b"] = json!(99.99);
        assert!(ticker_from_data(&d, 2, 3).is_none());
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
        let (stream, data) =
            parse_envelope(r#"{"stream":"btcusdt@ticker","data":{"s":"BTCUSDT"}}"#)
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
