use std::sync::Arc;

use broker::journal::EventKind;
use broker::{AccountSnapshot, Cents, ClosedDeal, Currency, PosSide};
use domain::account::AccountKind;
use persistence::{OutboxRecord, Persistence};
use serde::Serialize;

use crate::feed::FeedInstrument;

pub const DEFAULT_EXCHANGE: &str = "platform.events";

pub const SOURCE: &str = "terminal";

pub const EVENT_VERSION: u32 = 1;

pub const EV_ACCOUNT_OPENED: &str = "terminal.account.opened";
pub const EV_BALANCE_CHANGED: &str = "terminal.balance.changed";
pub const EV_TRADE_EXECUTED: &str = "terminal.trade.executed";
pub const EV_MARGIN_CALLED: &str = "terminal.margin.called";

pub const CURRENCY: &str = "USD";

pub const BATCH: i64 = 20;
pub const LEASE_MS: i64 = 30_000;
pub const POLL_MS: u64 = 1_000;

pub fn next_delay_ms(attempts: i32) -> i64 {
    const BASE: i64 = 5_000;
    const MAX: i64 = 3_600_000;
    let steps = attempts.max(1) - 1;
    let mut delay = BASE;
    for _ in 0..steps {
        delay = delay.saturating_mul(5);
        if delay >= MAX {
            return MAX;
        }
    }
    delay.min(MAX)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Envelope {
    pub event_id: String,
    pub event: String,
    pub version: u32,
    pub occurred_at: String,
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub data: serde_json::Value,
}

impl Envelope {
    pub fn to_record(&self) -> OutboxRecord {
        OutboxRecord {
            event_id: self.event_id.clone(),
            routing_key: self.event.clone(),
            payload: serde_json::to_string(self).expect("конверт сериализуем"),
        }
    }
}

pub fn event_id(kind: &str, key: &str) -> String {
    let name = format!("urn:terminal:{kind}:{key}");
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, name.as_bytes()).to_string()
}

pub fn account_uuid(tenant: &str, sub: &str, kind: AccountKind) -> String {
    let name = format!("urn:terminal:account:{tenant}:{sub}:{}", kind.tag());
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, name.as_bytes()).to_string()
}

pub fn iso_millis(at_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at_ms)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp_millis(0).expect("эпоха представима"))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn decimal(raw: i64, decimals: u8) -> String {
    if decimals == 0 {
        return raw.to_string();
    }
    let neg = raw < 0;
    let abs = (raw as i128).unsigned_abs();
    let p = 10u128.pow(decimals as u32);
    let (int, frac) = (abs / p, abs % p);
    let width = decimals as usize;
    let body = format!("{int}.{frac:0width$}");
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

fn num(dec: &str) -> serde_json::Value {
    serde_json::from_str(dec).unwrap_or(serde_json::Value::Null)
}

fn money(data: &mut serde_json::Value, prefix: &str, amount: Cents, currency: Currency) {
    data[format!("{prefix}Minor")] = cents(amount);
    data["currency"] = serde_json::Value::from(currency.code());
    data["currencyMinorUnits"] = serde_json::Value::from(currency.minor_units());
}

fn cents(v: i128) -> serde_json::Value {
    serde_json::Value::from(v.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
}

fn side_tag(s: PosSide) -> &'static str {
    match s {
        PosSide::Long => "buy",
        PosSide::Short => "sell",
    }
}

pub fn platform_symbol(f: &FeedInstrument) -> String {
    if !f.canonical.is_empty() {
        return f.canonical.clone();
    }
    let up: String = f
        .binance
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    match up.strip_suffix("USDT") {
        Some(base) if !base.is_empty() => format!("{base}USD"),
        _ => up,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Before {
    pub closed: usize,
    pub balance: Cents,
}

pub struct Addressee<'a> {
    pub tenant: &'a str,
    pub sub: &'a str,
    pub user: u64,
    pub seq: u64,
    pub at_ms: i64,
    pub kind: AccountKind,
    pub account_id: &'a str,
}

pub fn envelopes_for(
    who: &Addressee<'_>,
    before: &Before,
    after: &AccountSnapshot,
    kinds: &[EventKind],
    instrument: &dyn Fn(u32) -> Option<FeedInstrument>,
) -> Vec<Envelope> {
    let mut out = Vec::new();
    let occurred_at = iso_millis(who.at_ms);

    if let Some(EventKind::AccountOpened { balance, .. }) = kinds
        .iter()
        .find(|k| matches!(k, EventKind::AccountOpened { .. }))
    {
        let mut data = serde_json::json!({
            "tenant": who.tenant,
            "userId": who.sub,
            "accountSeq": who.seq,
            "accountKind": who.kind.tag(),
            "accountId": who.account_id,
            "currency": after.profile.currency.code(),
            "balanceCents": cents(*balance),
        });
        money(&mut data, "balance", *balance, after.profile.currency);
        out.push(Envelope {
            event_id: event_id(
                EV_ACCOUNT_OPENED,
                &match who.kind {
                    AccountKind::Demo => format!("{}:{}", who.tenant, who.sub),
                    AccountKind::Real => format!("{}:{}:real", who.tenant, who.sub),
                },
            ),
            event: EV_ACCOUNT_OPENED.to_string(),
            version: EVENT_VERSION,
            occurred_at: occurred_at.clone(),
            source: SOURCE,
            subject: Some(who.sub.to_string()),
            data,
        });
    }

    for (idx, deal) in after.closed.iter().enumerate().skip(before.closed) {
        let Some(f) = instrument(deal.instrument) else {
            eprintln!(
                "[bus] сделка по неизвестному инструменту {} — события не будет, символ выдумывать нельзя",
                deal.instrument
            );
            continue;
        };
        out.push(trade_executed(
            who,
            &occurred_at,
            idx,
            deal,
            &f,
            after.profile.currency,
        ));
    }

    if after.balance != before.balance {
        let reason = balance_reason(kinds, before, after);
        let mut data = serde_json::json!({
            "tenant": who.tenant,
            "userId": who.sub,
            "accountSeq": who.seq,
            "accountKind": who.kind.tag(),
            "accountId": who.account_id,
            "balanceCents": cents(after.balance),
            "positions": positions_json(after, instrument),
        });
        money(&mut data, "balance", after.balance, after.profile.currency);
        if let Some(r) = reason {
            match readable_reason(r) {
                Some(ok) => data["reason"] = serde_json::Value::from(ok),
                None => eprintln!(
                    "[bus] причина «{r}» потребителю неизвестна — событие уедет без неё, иначе потерялся бы и остаток"
                ),
            }
        }
        out.push(Envelope {
            event_id: event_id(EV_BALANCE_CHANGED, &format!("{}:{}", who.user, who.seq)),
            event: EV_BALANCE_CHANGED.to_string(),
            version: EVENT_VERSION,
            occurred_at,
            source: SOURCE,
            subject: Some(who.sub.to_string()),
            data,
        });
    }

    out
}

pub const CONSUMER_KNOWN_REASONS: [&str; 5] =
    ["trade", "deposit", "reset", "adjustment", "withdrawal"];

pub fn readable_reason(r: &str) -> Option<&str> {
    CONSUMER_KNOWN_REASONS.contains(&r).then_some(r)
}

fn balance_reason(
    kinds: &[EventKind],
    before: &Before,
    after: &AccountSnapshot,
) -> Option<&'static str> {
    if after.closed.len() > before.closed {
        return Some("trade");
    }
    if kinds
        .iter()
        .any(|k| matches!(k, EventKind::CommissionCharged { .. }))
    {
        return Some("trade");
    }
    if kinds
        .iter()
        .any(|k| matches!(k, EventKind::Deposited { .. }))
    {
        return Some("deposit");
    }
    if kinds
        .iter()
        .any(|k| matches!(k, EventKind::Withdrawn { .. }))
    {
        return Some("withdrawal");
    }
    if kinds
        .iter()
        .any(|k| matches!(k, EventKind::DemoAccountReset { .. }))
    {
        return Some("reset");
    }
    None
}

fn positions_json(
    snap: &AccountSnapshot,
    instrument: &dyn Fn(u32) -> Option<FeedInstrument>,
) -> serde_json::Value {
    let mut items = Vec::new();
    for p in &snap.positions {
        let Some(f) = instrument(p.instrument) else {
            eprintln!(
                "[bus] позиция по неизвестному инструменту {} — в снимок не попадёт",
                p.instrument
            );
            continue;
        };
        let mut item = serde_json::json!({
            "symbol": platform_symbol(&f),
            "side": side_tag(p.side),
            "volume": num(&decimal(p.qty, p.qd)),
            "entryPrice": num(&decimal(p.entry, p.pd)),
        });
        if let Some(sl) = p.sl {
            item["stopLoss"] = num(&decimal(sl, p.pd));
        }
        if let Some(tp) = p.tp {
            item["takeProfit"] = num(&decimal(tp, p.pd));
        }
        items.push(item);
    }
    serde_json::Value::Array(items)
}

fn trade_executed(
    who: &Addressee<'_>,
    occurred_at: &str,
    idx: usize,
    deal: &ClosedDeal,
    f: &FeedInstrument,
    currency: Currency,
) -> Envelope {
    let mut data = serde_json::json!({
        "tenant": who.tenant,
        "userId": who.sub,
        "accountSeq": who.seq,
        "accountKind": who.kind.tag(),
        "accountId": who.account_id,
        "tradeId": format!("{}:{}", who.user, idx),
        "symbol": platform_symbol(f),
        "side": side_tag(deal.side),
        "volume": num(&decimal(deal.qty, deal.qd)),
        "price": num(&decimal(deal.exit, deal.pd)),
        "executedAt": occurred_at,
        "realizedPnlCents": cents(deal.pnl),
    });
    money(&mut data, "realizedPnl", deal.pnl, currency);
    if let Some(level) = deal.level {
        data["level"] = num(&decimal(level, deal.pd));
    }
    if let Some(gap) = deal.across_gap {
        data["acrossGap"] = serde_json::Value::from(gap);
    }
    data["commissionMinor"] = cents(deal.fees.commission);
    data["markupMinor"] = cents(deal.fees.markup);
    if let Some(m) = deal.fees.market_exit {
        data["marketPrice"] = num(&decimal(m, deal.pd));
    }
    Envelope {
        event_id: event_id(EV_TRADE_EXECUTED, &format!("{}:{}", who.user, idx)),
        event: EV_TRADE_EXECUTED.to_string(),
        version: EVENT_VERSION,
        occurred_at: occurred_at.to_string(),
        source: SOURCE,
        subject: Some(format!("{}:{}", who.user, idx)),
        data,
    }
}

pub fn margin_called(
    who: &Addressee<'_>,
    level_pct: i128,
    stop_out_pct: Option<i128>,
    equity: Cents,
    used_margin: Cents,
) -> Envelope {
    let mut data = serde_json::json!({
        "tenant": who.tenant,
        "userId": who.sub,
        "accountSeq": who.seq,
        "accountKind": who.kind.tag(),
        "accountId": who.account_id,
        "marginLevelPct": cents(level_pct),
        "equityCents": cents(equity),
        "usedMarginCents": cents(used_margin),
    });
    if let Some(so) = stop_out_pct {
        data["stopOutLevelPct"] = cents(so);
    }
    Envelope {
        event_id: event_id(EV_MARGIN_CALLED, &format!("{}:{}", who.user, who.at_ms)),
        event: EV_MARGIN_CALLED.to_string(),
        version: EVENT_VERSION,
        occurred_at: iso_millis(who.at_ms),
        source: SOURCE,
        subject: Some(who.sub.to_string()),
        data,
    }
}

#[derive(Debug, Clone)]
pub struct BusError(pub String);

impl std::fmt::Display for BusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for BusError {}

#[allow(clippy::double_must_use)]
#[async_trait::async_trait]
pub trait BusPublisher: Send + Sync {
    async fn publish(
        &self,
        routing_key: &str,
        event_id: &str,
        payload: &str,
    ) -> Result<(), BusError>;
}

pub async fn relay_once(
    store: &dyn Persistence,
    publisher: &dyn BusPublisher,
    batch: i64,
) -> Result<usize, persistence::StoreError> {
    let claimed = store.claim_outbox(batch, LEASE_MS).await?;
    let mut sent = 0usize;
    for row in &claimed {
        match publisher
            .publish(&row.routing_key, &row.event_id, &row.payload)
            .await
        {
            Ok(()) => {
                store.mark_published(row.id).await?;
                sent += 1;
            }
            Err(e) => {
                let attempts = row.attempts + 1;
                let delay = next_delay_ms(attempts);
                eprintln!(
                    "[bus] отправка не удалась {} event_id={} попытка={attempts} повтор через {delay} мс: {e}",
                    row.routing_key, row.event_id
                );
                store.mark_failed(row.id, attempts, delay, &e.0).await?;
                break;
            }
        }
    }
    Ok(sent)
}

pub fn spawn_relay(store: Arc<dyn Persistence>, publisher: Arc<dyn BusPublisher>) {
    tokio::spawn(async move {
        loop {
            match relay_once(store.as_ref(), publisher.as_ref(), BATCH).await {
                Ok(n) if n as i64 >= BATCH => continue,
                Ok(_) => {}
                Err(e) => eprintln!("[bus] исходящая очередь недоступна: {e}"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
        }
    });
}

pub struct AmqpPublisher {
    url: String,
    exchange: String,
    link: tokio::sync::Mutex<Option<(lapin::Connection, lapin::Channel)>>,
}

impl AmqpPublisher {
    pub fn new(url: String, exchange: String) -> Self {
        Self {
            url,
            exchange,
            link: tokio::sync::Mutex::new(None),
        }
    }

    async fn channel(&self) -> Result<lapin::Channel, BusError> {
        let mut slot = self.link.lock().await;
        if let Some((conn, ch)) = slot.as_ref() {
            if conn.status().connected() && ch.status().connected() {
                return Ok(ch.clone());
            }
            *slot = None;
        }
        let conn = lapin::Connection::connect(&self.url, lapin::ConnectionProperties::default())
            .await
            .map_err(|e| BusError(format!("соединение с шиной: {e}")))?;
        let ch = conn
            .create_channel()
            .await
            .map_err(|e| BusError(format!("канал: {e}")))?;
        ch.exchange_declare(
            &self.exchange,
            lapin::ExchangeKind::Topic,
            lapin::options::ExchangeDeclareOptions {
                passive: true,
                durable: true,
                ..Default::default()
            },
            lapin::types::FieldTable::default(),
        )
        .await
        .map_err(|e| BusError(format!("обменник {}: {e}", self.exchange)))?;
        ch.confirm_select(lapin::options::ConfirmSelectOptions::default())
            .await
            .map_err(|e| BusError(format!("режим подтверждений: {e}")))?;
        *slot = Some((conn, ch.clone()));
        Ok(ch)
    }
}

#[async_trait::async_trait]
impl BusPublisher for AmqpPublisher {
    async fn publish(
        &self,
        routing_key: &str,
        event_id: &str,
        payload: &str,
    ) -> Result<(), BusError> {
        let ch = self.channel().await?;
        let props = lapin::BasicProperties::default()
            .with_delivery_mode(2)
            .with_content_type("application/json".into())
            .with_message_id(event_id.into());
        let confirm = ch
            .basic_publish(
                &self.exchange,
                routing_key,
                lapin::options::BasicPublishOptions::default(),
                payload.as_bytes(),
                props,
            )
            .await
            .map_err(|e| BusError(format!("публикация: {e}")))?
            .await
            .map_err(|e| BusError(format!("подтверждение: {e}")))?;
        if confirm.is_nack() {
            return Err(BusError("брокер не подтвердил сообщение (nack)".into()));
        }
        if matches!(
            confirm,
            lapin::publisher_confirm::Confirmation::NotRequested
        ) {
            return Err(BusError(
                "канал не в режиме подтверждений — отправку нечем подтвердить".into(),
            ));
        }
        Ok(())
    }
}

pub fn publisher_from_env() -> Option<Arc<dyn BusPublisher>> {
    let url = std::env::var("BUS_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())?;
    let exchange = std::env::var("BUS_EXCHANGE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_EXCHANGE.to_string());
    println!("[bus] публикация включена: обменник {exchange}");
    Some(Arc::new(AmqpPublisher::new(url, exchange)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use broker::journal::{CloseCause, EventKind};
    use broker::{AccountSnapshot, ClosedDeal, PosSide, Position, PositionModel};

    const TENANT: &str = "apex-ru";
    const SUB: &str = "user-42";

    fn btc() -> FeedInstrument {
        FeedInstrument {
            id: 1,
            symbol: "BTC-USDT".into(),
            binance: "BTCUSDT".into(),
            base: 1,
            quote: 100,
            price_decimals: 2,
            qty_decimals: 5,
            ..FeedInstrument::default()
        }
    }

    #[test]
    fn platform_symbol_of_the_thirty_pairs_is_unchanged_by_the_registry() {
        for f in crate::feed::instruments() {
            let before = platform_symbol(&FeedInstrument {
                canonical: String::new(),
                ..f.clone()
            });
            assert_eq!(platform_symbol(&f), before, "{}", f.binance);
        }
    }

    fn known(id: u32) -> Option<FeedInstrument> {
        (id == 1).then(btc)
    }

    fn empty_snap() -> AccountSnapshot {
        AccountSnapshot {
            profile: Default::default(),
            balance: 1_000_000,
            next_id: 1,
            positions: Vec::new(),
            pendings: Vec::new(),
            closed: Vec::new(),
            model: PositionModel::Hedging,
            leverage: None,
            class_leverage: Default::default(),
            fees: Default::default(),
        }
    }

    fn deal(level: Option<i64>, gap: Option<bool>) -> ClosedDeal {
        ClosedDeal {
            instrument: 1,
            side: PosSide::Long,
            qty: 150_000,
            entry: 6_700_000,
            exit: 6_823_412,
            pnl: 185_118,
            pd: 2,
            qd: 5,
            level,
            across_gap: gap,
            fees: Default::default(),
        }
    }

    fn who(seq: u64) -> Addressee<'static> {
        Addressee {
            tenant: TENANT,
            sub: SUB,
            user: 7,
            seq,
            at_ms: 1_767_225_600_123,
            kind: AccountKind::Demo,
            account_id: "00000000-0000-0000-0000-000000000000",
        }
    }

    fn after_one_deal(level: Option<i64>, gap: Option<bool>) -> AccountSnapshot {
        let mut s = empty_snap();
        s.balance = 1_185_118;
        s.closed = vec![deal(level, gap)];
        s
    }

    fn before_empty() -> Before {
        Before {
            closed: 0,
            balance: 1_000_000,
        }
    }

    #[test]
    fn every_account_event_carries_the_journal_position() {
        let evs = envelopes_for(
            &who(42),
            &before_empty(),
            &after_one_deal(None, None),
            &[EventKind::AccountOpened {
                balance: 1_000_000,
                profile: Default::default(),
            }],
            &known,
        );
        assert!(
            evs.len() >= 3,
            "ожидались заведение, сделка и остаток: {evs:?}"
        );
        for e in &evs {
            assert_eq!(
                e.data["accountSeq"],
                serde_json::json!(42),
                "событие {} уехало без позиции в журнале: {:?}",
                e.event,
                e.data
            );
        }
    }

    #[test]
    fn one_batch_means_one_number_for_all_its_events() {
        let evs = envelopes_for(
            &who(7),
            &before_empty(),
            &after_one_deal(None, None),
            &[EventKind::AccountOpened {
                balance: 1_000_000,
                profile: Default::default(),
            }],
            &known,
        );
        let nums: Vec<&serde_json::Value> = evs.iter().map(|e| &e.data["accountSeq"]).collect();
        assert!(
            nums.windows(2).all(|w| w[0] == w[1]),
            "в одной пачке номера разошлись: {nums:?}"
        );
    }

    #[test]
    fn a_batch_carries_at_most_one_balance_event() {
        let evs = envelopes_for(
            &who(7),
            &before_empty(),
            &after_one_deal(None, None),
            &[EventKind::AccountOpened {
                balance: 1_000_000,
                profile: Default::default(),
            }],
            &known,
        );
        let n = evs.iter().filter(|e| e.event == EV_BALANCE_CHANGED).count();
        assert_eq!(
            n, 1,
            "событий об остатке в пачке должно быть ровно одно, их {n}: {evs:?}"
        );
    }

    fn trade_of(evs: &[Envelope]) -> &Envelope {
        evs.iter()
            .find(|e| e.event == EV_TRADE_EXECUTED)
            .expect("событие о сделке обязано быть")
    }

    #[test]
    fn price_and_volume_travel_as_decimals() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, Some(false)),
            &[],
            &known,
        );
        let d = &trade_of(&evs).data;
        assert_eq!(
            d["price"],
            serde_json::json!(68_234.12),
            "цена в валюте, не в единицах точности"
        );
        assert_eq!(
            d["volume"],
            serde_json::json!(1.5),
            "объём в лотах, не в единицах точности"
        );
        assert_eq!(
            d["realizedPnlCents"],
            serde_json::json!(185_118),
            "деньги остаются целыми центами — их схема так и объявляет"
        );
    }

    #[test]
    fn decimal_is_exact_at_the_edges() {
        assert_eq!(decimal(6_823_412, 2), "68234.12");
        assert_eq!(decimal(1, 8), "0.00000001");
        assert_eq!(decimal(-1, 2), "-0.01");
        assert_eq!(decimal(0, 5), "0.00000");
        assert_eq!(decimal(123, 0), "123", "ноль знаков — целое остаётся целым");
        assert_eq!(
            decimal(i64::MAX, 2),
            "92233720368547758.07",
            "предел не переполняет"
        );
    }

    #[test]
    fn gap_flag_is_camel_case_on_the_wire() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(Some(6_800_000), Some(true)),
            &[],
            &known,
        );
        let d = &trade_of(&evs).data;
        assert_eq!(d["acrossGap"], serde_json::json!(true));
        assert!(
            d.get("across_gap").is_none(),
            "внутреннее имя за границу не выходит"
        );
        assert_eq!(
            d["level"],
            serde_json::json!(68_000.0),
            "уровень — в тех же единицах, что цена"
        );
    }

    #[test]
    fn unknown_level_and_gap_are_absent_not_false() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        let d = &trade_of(&evs).data;
        assert!(d.get("level").is_none(), "уровня не было — поля нет");
        assert!(
            d.get("acrossGap").is_none(),
            "про разрыв неизвестно — поля нет, а не false"
        );
    }

    #[test]
    fn trade_without_optional_fields_is_still_complete() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        let d = &trade_of(&evs).data;
        for key in [
            "tenant",
            "userId",
            "tradeId",
            "symbol",
            "side",
            "volume",
            "price",
            "executedAt",
        ] {
            assert!(d.get(key).is_some(), "обязательное поле {key} пропало");
        }
    }

    #[test]
    fn envelope_addresses_the_site_and_its_user() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        let e = trade_of(&evs);
        assert_eq!(e.event, EV_TRADE_EXECUTED);
        assert_eq!(e.version, EVENT_VERSION);
        assert_eq!(e.source, SOURCE);
        assert_eq!(e.occurred_at, "2026-01-01T00:00:00.123Z");
        assert_eq!(e.data["tenant"], TENANT);
        assert_eq!(
            e.data["userId"], SUB,
            "адресат — пользователь САЙТА, не наш номер счёта"
        );
        assert_eq!(
            e.data["symbol"], "BTCUSD",
            "написание платформы, не ETH-USDT и не номер"
        );
        assert_eq!(
            e.data["side"], "buy",
            "сторона СДЕЛКИ, а не закрывающей ноги"
        );
    }

    #[test]
    fn event_id_is_derived_from_the_fact() {
        let once = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        let twice = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        assert_eq!(trade_of(&once).event_id, trade_of(&twice).event_id);
        assert_ne!(
            event_id(EV_TRADE_EXECUTED, "7:0"),
            event_id(EV_TRADE_EXECUTED, "7:1"),
            "разные сделки — разные идентификаторы"
        );
        assert_ne!(
            event_id(EV_TRADE_EXECUTED, "7:0"),
            event_id(EV_BALANCE_CHANGED, "7:0"),
            "разные события одного счёта не сливаются"
        );
        assert_eq!(
            event_id(EV_TRADE_EXECUTED, "7:0"),
            "f34e50af-4fa9-5d60-9167-a84bfd8c5a60"
        );
    }

    #[test]
    fn account_opened_comes_from_the_journal_not_from_guessing() {
        let evs = envelopes_for(
            &who(1),
            &before_empty(),
            &empty_snap(),
            &[EventKind::AccountOpened {
                balance: 1_000_000,
                profile: Default::default(),
            }],
            &known,
        );
        let e = evs
            .iter()
            .find(|e| e.event == EV_ACCOUNT_OPENED)
            .expect("заведение счёта обязано уехать");
        assert_eq!(e.data["balanceCents"], serde_json::json!(1_000_000));
        assert_eq!(e.data["currency"], "USD");
    }

    #[test]
    fn balance_change_carries_the_positions_snapshot() {
        let mut after = after_one_deal(None, Some(false));
        after.positions = vec![Position {
            id: 2,
            instrument: 1,
            side: PosSide::Short,
            qty: 50_000,
            entry: 6_900_000,
            pd: 2,
            qd: 5,
            margin: 100_000,
            sl: None,
            tp: None,
            leverage: 1,
            fees: Default::default(),
        }];
        let evs = envelopes_for(&who(9), &before_empty(), &after, &[], &known);
        let e = evs
            .iter()
            .find(|e| e.event == EV_BALANCE_CHANGED)
            .expect("остаток изменился — событие обязано быть");
        assert_eq!(e.data["balanceCents"], serde_json::json!(1_185_118));
        assert_eq!(e.data["reason"], "trade");
        let p = &e.data["positions"][0];
        assert_eq!(p["symbol"], "BTCUSD");
        assert_eq!(p["side"], "sell");
        assert_eq!(p["volume"], serde_json::json!(0.5));
        assert_eq!(p["entryPrice"], serde_json::json!(69_000.0));
        assert!(
            p.get("stopLoss").is_none() && p.get("takeProfit").is_none(),
            "уровней нет — полей нет: {p}"
        );
    }

    #[test]
    fn positions_snapshot_carries_levels_as_additive_fields() {
        let mut after = after_one_deal(None, Some(false));
        after.positions = vec![Position {
            id: 2,
            instrument: 1,
            side: PosSide::Long,
            qty: 50_000,
            entry: 6_900_000,
            pd: 2,
            qd: 5,
            margin: 100_000,
            sl: Some(6_850_050),
            tp: None,
            leverage: 1,
            fees: Default::default(),
        }];
        let evs = envelopes_for(&who(9), &before_empty(), &after, &[], &known);
        let e = evs.iter().find(|e| e.event == EV_BALANCE_CHANGED).unwrap();
        let p = &e.data["positions"][0];
        let keys: Vec<&String> = p.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            ["entryPrice", "side", "stopLoss", "symbol", "volume"],
            "прежние четыре поля на месте, добавлено одно"
        );
        assert_eq!(p["stopLoss"], serde_json::json!(68_500.5));
        assert_eq!(p["entryPrice"], serde_json::json!(69_000.0));
    }

    #[test]
    fn unchanged_balance_produces_no_event() {
        let evs = envelopes_for(&who(3), &before_empty(), &empty_snap(), &[], &known);
        assert!(evs.iter().all(|e| e.event != EV_BALANCE_CHANGED));
    }

    #[test]
    fn reason_is_omitted_when_unknown() {
        let mut after = empty_snap();
        after.balance = 900_000;
        let evs = envelopes_for(&who(4), &before_empty(), &after, &[], &known);
        let e = evs
            .iter()
            .find(|e| e.event == EV_BALANCE_CHANGED)
            .expect("остаток изменился");
        assert!(e.data.get("reason").is_none(), "причина не выдумывается");
    }

    #[test]
    fn every_reason_we_can_emit_is_one_the_consumer_knows() {
        let before = before_empty();
        let opened = [EventKind::AccountOpened {
            balance: 1,
            profile: Default::default(),
        }];
        let deposited = [EventKind::Deposited { amount: 1 }];
        let traded = after_one_deal(None, None);
        let mut moved = empty_snap();
        moved.balance = 1;

        let cases: [(&[EventKind], &AccountSnapshot); 4] = [
            (&[], &moved),
            (&opened, &moved),
            (&deposited, &moved),
            (&[], &traded),
        ];
        for (kinds, after) in cases {
            if let Some(r) = balance_reason(kinds, &before, after) {
                assert!(
                    CONSUMER_KNOWN_REASONS.contains(&r),
                    "причина «{r}» потребителю неизвестна — событие станет malformed, и остаток потеряется"
                );
            }
        }
    }
    #[test]
    fn an_unknown_reason_is_not_put_on_the_wire() {
        assert_eq!(
            readable_reason("trade"),
            Some("trade"),
            "известную пропускаем"
        );
        assert_eq!(
            readable_reason("compensation"),
            None,
            "а незнакомую — нет: она уронила бы разбор всего события, и остаток потерялся бы вместе с причиной"
        );
        assert_eq!(readable_reason(""), None);
        assert_eq!(
            readable_reason("TRADE"),
            None,
            "перечень точный, а не похожий"
        );
    }

    #[test]
    fn money_travels_as_amount_currency_and_precision() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, Some(false)),
            &[],
            &known,
        );
        let b = evs
            .iter()
            .find(|e| e.event == EV_BALANCE_CHANGED)
            .expect("событие об остатке");
        assert_eq!(
            b.data["balanceMinor"], b.data["balanceCents"],
            "то же число"
        );
        assert_eq!(b.data["currency"], "USD");
        assert_eq!(b.data["currencyMinorUnits"], 2);

        let t = trade_of(&evs);
        assert_eq!(t.data["realizedPnlMinor"], t.data["realizedPnlCents"]);
        assert_eq!(t.data["currencyMinorUnits"], 2);
    }

    #[test]
    fn the_old_field_is_still_sent() {
        let evs = envelopes_for(
            &who(1),
            &before_empty(),
            &empty_snap(),
            &[EventKind::AccountOpened {
                balance: 1_000_000,
                profile: Default::default(),
            }],
            &known,
        );
        let e = evs
            .iter()
            .find(|e| e.event == EV_ACCOUNT_OPENED)
            .expect("заведение счёта");
        assert_eq!(e.data["balanceCents"], serde_json::json!(1_000_000));
        assert_eq!(e.data["balanceMinor"], serde_json::json!(1_000_000));
    }

    #[test]
    fn a_demo_reset_is_reported_as_reset() {
        let mut after = empty_snap();
        after.balance = 1_000_000;
        let evs = envelopes_for(
            &who(7),
            &Before {
                closed: 0,
                balance: 900_000,
            },
            &after,
            &[EventKind::DemoAccountReset {
                adjustment: 100_000,
            }],
            &known,
        );
        let e = evs
            .iter()
            .find(|e| e.event == EV_BALANCE_CHANGED)
            .expect("остаток изменился");
        assert_eq!(e.data["reason"], "reset");
        assert!(
            CONSUMER_KNOWN_REASONS.contains(&"reset"),
            "и эта причина потребителю известна"
        );
    }

    #[test]
    fn unknown_instrument_yields_no_trade_event() {
        let mut after = after_one_deal(None, None);
        after.closed[0].instrument = 999;
        let evs = envelopes_for(&who(5), &before_empty(), &after, &[], &known);
        assert!(
            evs.iter().all(|e| e.event != EV_TRADE_EXECUTED),
            "символ выдумывать нельзя"
        );
    }

    #[test]
    fn backoff_grows_and_stops_at_an_hour() {
        assert_eq!(next_delay_ms(1), 5_000);
        assert_eq!(next_delay_ms(2), 25_000);
        assert_eq!(next_delay_ms(3), 125_000);
        assert_eq!(next_delay_ms(4), 625_000);
        assert_eq!(
            next_delay_ms(5),
            3_125_000,
            "пятая ещё не упёрлась в потолок"
        );
        assert_eq!(next_delay_ms(6), 3_600_000, "шестая — потолок, час");
        assert_eq!(
            next_delay_ms(50),
            3_600_000,
            "и дальше час, а не переполнение"
        );
        assert_eq!(
            next_delay_ms(0),
            5_000,
            "нулевая попытка не уводит отступ в ноль"
        );
    }

    #[test]
    fn iso_is_millisecond_utc() {
        assert_eq!(iso_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_millis(1_767_225_600_123), "2026-01-01T00:00:00.123Z");
    }

    #[test]
    fn platform_symbol_matches_the_allow_list_spelling() {
        assert_eq!(platform_symbol(&btc()), "BTCUSD");
    }

    #[test]
    fn close_cause_stays_inside() {
        let _ = CloseCause::StopLoss;
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        assert!(trade_of(&evs).data.get("cause").is_none());
    }
}
