use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::feed::{self, Depth, FeedInstrument, Kline, Source, Ticker};
use crate::instruments::{MarketState, Status, V2Endpoint, V2_SILENCE_RECONNECT_SECS};
use crate::AppState;

pub const SUBSCRIBE_CHUNK: usize = 100;
pub const SNAPSHOT_CHUNK: usize = 50;
pub const DAILY_REFRESH_SECS: u64 = 60;

pub fn interval(tf: u32) -> Option<&'static str> {
    match tf {
        1 => Some("1s"),
        60 => Some("1m"),
        300 => Some("5m"),
        900 => Some("15m"),
        1800 => Some("30m"),
        3600 => Some("1h"),
        14400 => Some("4h"),
        86400 => Some("1d"),
        604800 => Some("1w"),
        _ => None,
    }
}

async fn get_json(http: &reqwest::Client, url: &str, base: &str) -> Result<Value, String> {
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string().replace(base, &feed::masked(base)))?;
    if !resp.status().is_success() {
        return Err(format!("модуль v2 {}", resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

pub fn candles_from_rows(rows: &[Value], pd: u8, qd: u8) -> Result<Vec<Kline>, String> {
    let raw_at = |r: &[Value], i: usize, dec: u8| -> Result<i64, String> {
        let s = r
            .get(i)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("нет поля {i}"))?;
        feed::to_raw(s, dec).ok_or_else(|| format!("поле {i}: не число ({s})"))
    };
    rows.iter()
        .map(|row| {
            let r = row.as_array().ok_or("свеча не массив")?;
            let volume = match r.get(5) {
                None | Some(Value::Null) => None,
                Some(_) => Some(raw_at(r, 5, qd)?),
            };
            Ok(Kline {
                time: r
                    .first()
                    .and_then(Value::as_i64)
                    .ok_or("нет времени свечи")?
                    / 1000,
                open: raw_at(r, 1, pd)?,
                high: raw_at(r, 2, pd)?,
                low: raw_at(r, 3, pd)?,
                close: raw_at(r, 4, pd)?,
                volume,
            })
        })
        .collect()
}

pub async fn fetch_candles(
    http: &reqwest::Client,
    v2: &V2Endpoint,
    f: &FeedInstrument,
    interval: &str,
    limit: usize,
    end: Option<i64>,
) -> Result<Vec<Kline>, String> {
    let mut url = format!(
        "{}/v2/candles?symbol={}&interval={interval}&limit={limit}",
        v2.rest, f.binance
    );
    if let Some(end) = end {
        url.push_str(&format!("&end={end}"));
    }
    let v = get_json(http, &url, &v2.rest).await?;
    let rows = v.as_array().ok_or("ответ свечей не массив")?;
    candles_from_rows(rows, f.price_decimals, f.qty_decimals)
}

pub async fn fetch_depth(
    http: &reqwest::Client,
    v2: &V2Endpoint,
    f: &FeedInstrument,
    limit: usize,
) -> Result<Depth, String> {
    let url = format!("{}/v2/depth?symbol={}&limit={limit}", v2.rest, f.binance);
    let v = get_json(http, &url, &v2.rest).await?;
    let (depth, skipped) = feed::depth_from_json(&v, f.price_decimals, f.qty_decimals);
    if skipped > 0 {
        eprintln!(
            "[feed-v2] {}: пропущено уровней стакана с неразобранными числами: {skipped}",
            f.canonical
        );
    }
    Ok(depth)
}

pub async fn fetch_change(
    http: &reqwest::Client,
    v2: &V2Endpoint,
    symbol: &str,
    interval: &str,
) -> Result<f64, String> {
    let url = format!(
        "{}/v2/candles?symbol={symbol}&interval={interval}&limit=1",
        v2.rest
    );
    let v = get_json(http, &url, &v2.rest).await?;
    let row = v
        .as_array()
        .and_then(|a| a.last())
        .and_then(Value::as_array)
        .ok_or("пусто")?;
    feed::change_from_row(row)
}

pub(crate) type Index = HashMap<String, (u32, u8, u8, bool)>;

pub(crate) fn index_of(st: &AppState) -> Index {
    st.catalog_snapshot()
        .into_iter()
        .filter(|f| f.source == Source::V2)
        .map(|f| {
            (
                f.binance.clone(),
                (f.id, f.price_decimals, f.qty_decimals, f.has_trades),
            )
        })
        .collect()
}

fn op(kind: &str, symbols: &[String], id: u64) -> String {
    serde_json::json!({ "op": kind, "symbols": symbols, "id": id }).to_string()
}

pub fn ticker_from_quote(
    bid: i64,
    ask: i64,
    daily: Option<(i64, i64, i64)>,
    prev: Option<&Ticker>,
) -> Ticker {
    let mid = (bid + ask) / 2;
    let (change, high, low) = match daily {
        Some((open, high, low)) if open > 0 => (
            (mid - open) as f64 / open as f64 * 100.0,
            high.max(mid),
            Some(low.min(mid)),
        ),
        _ => (f64::NAN, prev.map(|p| p.high.max(mid)).unwrap_or(mid), None),
    };
    Ticker {
        last: mid,
        change,
        high,
        bid,
        ask,
        low,
        volume: None,
        quote_volume: None,
    }
}

fn market_event(id: u32, m: &MarketState) -> String {
    serde_json::json!([{
        "type": "market",
        "instrument": id,
        "status": m.status.tag(),
        "next_open": m.next_open,
        "reason": m.reason,
    }])
    .to_string()
}

pub async fn handle_message(st: &AppState, index: &Index, text: &str) {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[feed-v2] нераспознанное сообщение ({e}): {text:.120}");
            return;
        }
    };
    st.mark_v2_alive();
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    let sym = v.get("s").and_then(Value::as_str).unwrap_or("");
    let found = index.get(sym).copied();
    match (kind, found) {
        ("quote", Some((id, pd, _, _))) => {
            let raw = |k: &str| {
                v.get(k)
                    .and_then(Value::as_str)
                    .and_then(|s| feed::to_raw(s, pd))
            };
            let (Some(bid), Some(ask)) = (raw("b"), raw("a")) else {
                eprintln!("[feed-v2] {sym}: котировка без разборчивых bid/ask — пропущена");
                return;
            };
            if bid <= 0 || ask <= 0 || ask < bid {
                eprintln!(
                    "[feed-v2] {sym}: котировка bid {bid} / ask {ask} невозможна — пропущена"
                );
                return;
            }
            let daily = st.v2_daily_of(id);
            let ticker = {
                let mut t = st.tickers.lock().await;
                let next = ticker_from_quote(bid, ask, daily, t.get(&id));
                t.insert(id, next);
                next
            };
            let now = crate::now_ms();
            let reopened = {
                let mut m = st.markets.lock().unwrap_or_else(|e| e.into_inner());
                let state = m
                    .entry(id)
                    .or_insert_with(|| MarketState::new(Status::Open));
                let was = state.status;
                state.status = Status::Open;
                state.next_open = None;
                state.reason = None;
                state.last_quote_ms = now;
                (was != Status::Open).then(|| state.clone())
            };
            if let Some(state) = reopened {
                let _ = st.events_tx.send(market_event(id, &state));
            }
            let msg = serde_json::json!([{
                "type": "quote",
                "instrument": id,
                "bid": bid,
                "ask": ask,
                "price": ticker.last,
                "time": v.get("t").and_then(Value::as_i64),
            }])
            .to_string();
            let _ = st.events_tx.send(msg);
        }
        ("trade", Some((id, pd, qd, has_trades))) => {
            if !has_trades {
                return;
            }
            let price = v
                .get("p")
                .and_then(Value::as_str)
                .and_then(|s| feed::to_raw(s, pd));
            let qty = v
                .get("q")
                .and_then(Value::as_str)
                .and_then(|s| feed::to_raw(s, qd));
            let side = match v.get("side").and_then(Value::as_str) {
                Some("sell") => "sell",
                _ => "buy",
            };
            if let (Some(price), Some(qty)) = (price, qty) {
                let msg = serde_json::json!([{
                    "type": "trade", "instrument": id, "price": price, "qty": qty,
                    "taker_side": side, "time": v.get("t").and_then(Value::as_i64)
                }])
                .to_string();
                let _ = st.events_tx.send(msg);
            }
        }
        ("status", Some((id, _, _, _))) => {
            let Some(status) = v
                .get("status")
                .and_then(Value::as_str)
                .and_then(Status::parse)
            else {
                eprintln!("[feed-v2] {sym}: неизвестный статус рынка — пропущен");
                return;
            };
            let changed = {
                let mut m = st.markets.lock().unwrap_or_else(|e| e.into_inner());
                let state = m.entry(id).or_insert_with(|| MarketState::new(status));
                let before = state.clone();
                state.status = status;
                state.next_open = v.get("nextOpen").and_then(Value::as_i64);
                state.reason = v.get("reason").and_then(Value::as_str).map(str::to_string);
                if status != Status::Open {
                    state.gap = true;
                }
                (before != *state).then(|| state.clone())
            };
            if let Some(state) = changed {
                println!("[feed-v2] {sym}: рынок {}", state.status.tag());
                let _ = st.events_tx.send(market_event(id, &state));
            }
        }
        ("error", _) => eprintln!(
            "[feed-v2] модуль ответил ошибкой: {}",
            v.get("code").and_then(Value::as_str).unwrap_or("?")
        ),
        _ => {}
    }
}

async fn snapshot(st: &AppState, v2: &V2Endpoint, index: &Index, symbols: &[String]) {
    for chunk in symbols.chunks(SNAPSHOT_CHUNK) {
        let url = format!("{}/v2/quotes?symbols={}", v2.rest, chunk.join(","));
        let v = match get_json(&st.platform.http, &url, &v2.rest).await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[feed-v2] снимок котировок не получен: {e}");
                return;
            }
        };
        for q in v
            .get("quotes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(sym) = q.get("s").and_then(Value::as_str) else {
                continue;
            };
            let Some(&(id, pd, _, _)) = index.get(sym) else {
                continue;
            };
            let raw = |k: &str| {
                q.get(k)
                    .and_then(Value::as_str)
                    .and_then(|s| feed::to_raw(s, pd))
            };
            if let (Some(bid), Some(ask)) = (raw("b"), raw("a")) {
                if bid > 0 && ask >= bid {
                    let daily = st.v2_daily_of(id);
                    let mut t = st.tickers.lock().await;
                    let next = ticker_from_quote(bid, ask, daily, t.get(&id));
                    t.entry(id).or_insert(next);
                }
            }
            if let Some(status) = q
                .get("status")
                .and_then(Value::as_str)
                .and_then(Status::parse)
            {
                let mut m = st.markets.lock().unwrap_or_else(|e| e.into_inner());
                let state = m.entry(id).or_insert_with(|| MarketState::new(status));
                state.status = status;
                if status != Status::Open {
                    state.gap = true;
                }
            }
        }
    }
}

pub fn spawn(st: AppState) {
    let Some(v2) = st.platform.v2.clone() else {
        return;
    };
    let daily = st.clone();
    let daily_v2 = v2.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(DAILY_REFRESH_SECS));
        loop {
            tick.tick().await;
            refresh_daily(&daily, &daily_v2).await;
        }
    });
    tokio::spawn(async move {
        let mut subs = st.platform.subscriptions_v2();
        let mut next_id: u64 = 1;
        loop {
            let wanted = subs.borrow_and_update().clone();
            if wanted.is_empty() {
                if subs.changed().await.is_err() {
                    return;
                }
                continue;
            }
            let mut ws = match connect_async(&v2.ws).await {
                Ok((ws, _)) => ws,
                Err(e) => {
                    eprintln!(
                        "[feed-v2] ошибка подключения: {}",
                        e.to_string().replace(&v2.ws, &feed::masked(&v2.ws))
                    );
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
            };
            println!(
                "[feed-v2] модуль v2 подключён ({}), инструментов {}",
                feed::masked(&v2.ws),
                wanted.len()
            );
            let mut index = index_of(&st);
            let list: Vec<String> = wanted.iter().cloned().collect();
            let mut ok = true;
            for chunk in list.chunks(SUBSCRIBE_CHUNK) {
                if ws
                    .send(Message::Text(op("subscribe", chunk, next_id)))
                    .await
                    .is_err()
                {
                    ok = false;
                    break;
                }
                next_id += 1;
            }
            if !ok {
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            st.mark_v2_alive();
            snapshot(&st, &v2, &index, &list).await;
            let mut current: BTreeSet<String> = wanted;
            let mut silence = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    msg = ws.next() => match msg {
                        Some(Ok(Message::Text(t))) => handle_message(&st, &index, &t).await,
                        Some(Ok(Message::Ping(p))) => {
                            let _ = ws.send(Message::Pong(p)).await;
                        }
                        Some(Ok(Message::Close(f))) => {
                            eprintln!("[feed-v2] соединение закрыто модулем: {f:?}");
                            break;
                        }
                        Some(Err(e)) => {
                            eprintln!("[feed-v2] обрыв соединения: {e}");
                            break;
                        }
                        None => break,
                        _ => {}
                    },
                    changed = subs.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        let next = subs.borrow_and_update().clone();
                        let added: Vec<String> = next.difference(&current).cloned().collect();
                        let removed: Vec<String> = current.difference(&next).cloned().collect();
                        index = index_of(&st);
                        let mut failed = false;
                        for chunk in removed.chunks(SUBSCRIBE_CHUNK) {
                            failed |= ws.send(Message::Text(op("unsubscribe", chunk, next_id))).await.is_err();
                            next_id += 1;
                        }
                        for chunk in added.chunks(SUBSCRIBE_CHUNK) {
                            failed |= ws.send(Message::Text(op("subscribe", chunk, next_id))).await.is_err();
                            next_id += 1;
                        }
                        if !added.is_empty() || !removed.is_empty() {
                            println!("[feed-v2] подписка дельтой: +{} −{}", added.len(), removed.len());
                        }
                        if !added.is_empty() {
                            snapshot(&st, &v2, &index, &added).await;
                        }
                        current = next;
                        if failed {
                            break;
                        }
                    }
                    _ = silence.tick() => {
                        if st.v2_silent_for_secs() > V2_SILENCE_RECONNECT_SECS as i64 {
                            eprintln!("[feed-v2] поток молчит дольше {V2_SILENCE_RECONNECT_SECS} с — переподключение");
                            break;
                        }
                    }
                }
            }
            eprintln!("[feed-v2] переподключение через 3с");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}

async fn refresh_daily(st: &AppState, v2: &V2Endpoint) {
    let wanted = st.platform.subscriptions_v2().borrow().clone();
    let list: Vec<FeedInstrument> = st
        .catalog_snapshot()
        .into_iter()
        .filter(|f| f.source == Source::V2 && wanted.contains(&f.binance))
        .collect();
    for f in list {
        match fetch_candles(&st.platform.http, v2, &f, "1d", 1, None).await {
            Ok(rows) => {
                if let Some(k) = rows.last() {
                    st.set_v2_daily(f.id, (k.open, k.high, k.low));
                }
            }
            Err(e) => eprintln!("[feed-v2] дневная свеча {} не получена: {e}", f.canonical),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn candles_keep_missing_volume_missing() {
        let rows = vec![
            json!([
                1_795_078_800_000i64,
                "1.08401",
                "1.08455",
                "1.08390",
                "1.08412",
                null
            ]),
            json!([
                1_795_082_400_000i64,
                "1.08412",
                "1.08500",
                "1.08400",
                "1.08450",
                "1532"
            ]),
        ];
        let out = candles_from_rows(&rows, 5, 0).unwrap();
        assert_eq!(out[0].volume, None);
        assert_eq!(out[1].volume, Some(1532));
        assert_eq!(out[0].close, 108_412);
        let bad = vec![json!([1i64, "x", "1", "1", "1", null])];
        assert!(candles_from_rows(&bad, 5, 0).is_err());
    }

    #[test]
    fn quote_ticker_uses_mid_and_unknown_change_without_daily() {
        let t = ticker_from_quote(108_412, 108_415, None, None);
        assert_eq!((t.bid, t.ask, t.last), (108_412, 108_415, 108_413));
        assert!(t.change.is_nan());
        let t = ticker_from_quote(108_412, 108_416, Some((108_000, 108_500, 107_900)), None);
        assert!((t.change - (414.0 / 108_000.0 * 100.0)).abs() < 1e-9);
        assert_eq!((t.high, t.low), (108_500, Some(107_900)));
    }

    #[test]
    fn intervals_are_the_contract_set() {
        assert_eq!(interval(60), Some("1m"));
        assert_eq!(interval(604800), Some("1w"));
        assert_eq!(interval(7), None);
    }
}
