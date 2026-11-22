pub mod bus;
mod dec;
pub mod exclusion;
pub mod feed;
pub mod feed_v2;
pub mod instruments;
pub mod jurisdiction;
mod levels;
mod money;
mod sso;
mod stops;
#[cfg(test)]
mod wave2_tests;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};

use domain::account::{AccountKind, AccountProfile, UserId};

use broker::journal::{self, Journal, Origin};
use broker::{Broker, PosSide};
use persistence::{MemoryStore, Persistence};

pub use persistence;

const DEFAULT_USER: UserId = UserId(1);

type KlineCache = HashMap<(u32, u32), (Instant, Vec<feed::Kline>)>;
type DepthCache = HashMap<(u32, usize), (Instant, feed::Depth)>;
type StatsCache = HashMap<u32, (Instant, Vec<StatDto>)>;
type DailyCache = HashMap<u32, (i64, i64, i64)>;
type IdentityCache = HashMap<UserId, Option<BusIdentity>>;

#[derive(Clone)]
struct BusIdentity {
    tenant: String,
    sub: String,
    kind: AccountKind,
}

const STAT_TFS: [u32; 8] = [60, 300, 900, 1800, 3600, 14400, 86400, 604800];

#[derive(Clone)]
pub struct AppState {
    events_tx: broadcast::Sender<String>,
    catalog: Arc<std::sync::RwLock<instruments::Catalog>>,
    markets: Arc<std::sync::Mutex<HashMap<u32, instruments::MarketState>>>,
    platform: Arc<instruments::Platform>,
    v2_last_msg: Arc<std::sync::atomic::AtomicI64>,
    v2_daily: Arc<std::sync::Mutex<DailyCache>>,
    tickers: Arc<Mutex<HashMap<u32, feed::Ticker>>>,
    feed_last_msg: Arc<std::sync::atomic::AtomicI64>,
    feed_gap_pending: Arc<std::sync::atomic::AtomicBool>,
    klines_cache: Arc<Mutex<KlineCache>>,
    depth_cache: Arc<Mutex<DepthCache>>,
    stats_cache: Arc<Mutex<StatsCache>>,
    broker: Arc<Mutex<Broker>>,
    store: Arc<dyn Persistence>,
    acct_locks: Arc<Mutex<HashMap<UserId, Arc<Mutex<()>>>>>,
    quarantined: Arc<Mutex<std::collections::HashSet<UserId>>>,
    journals: Arc<Mutex<HashMap<UserId, Journal>>>,
    tenant_instruments: Arc<TenantSource>,
    sso: Arc<sso::Sso>,
    sso_enabled: bool,
    identities: Arc<Mutex<IdentityCache>>,
    bus_enabled: bool,
    internal_money_token: Option<Arc<str>>,
    money_lock: Arc<Mutex<()>>,
    real_trading_enabled: bool,
    sltp_edit_enabled: bool,
    leverage_classes_enabled: bool,
    bus_real_events: bool,
    stops: Arc<stops::StopSettings>,
}

pub fn build_state() -> AppState {
    build_state_with(Arc::new(MemoryStore::new()))
}

pub fn build_state_with(store: Arc<dyn Persistence>) -> AppState {
    let (events_tx, _rx) = broadcast::channel(4096);
    let jwks_url = std::env::var("CABINET_URL")
        .ok()
        .map(|b| b.trim_end_matches('/').to_string())
        .filter(|b| !b.is_empty())
        .map(|b| format!("{b}/api/sso/jwks"))
        .unwrap_or_default();
    let sso = Arc::new(sso::Sso::new(jwks_url));
    let sso_enabled = sso.enabled() && std::env::var("SSO_DISABLED").ok().as_deref() != Some("1");
    let start_cents =
        demo_start_from_env(std::env::var("DEMO_START_BALANCE_CENTS").ok().as_deref());
    let stop_out_pct = std::env::var("MARGIN_STOP_OUT_LEVEL_PERCENT")
        .ok()
        .and_then(|s| s.trim().parse::<i128>().ok())
        .filter(|p| *p > 0);
    let margin_call_pct = std::env::var("MARGIN_CALL_LEVEL_PERCENT")
        .ok()
        .and_then(|s| s.trim().parse::<i128>().ok())
        .filter(|p| *p > 0);
    let protect_negative = matches!(
        std::env::var("NEGATIVE_BALANCE_PROTECTION").ok().as_deref(),
        Some("1") | Some("true")
    );
    if protect_negative {
        println!(
            "[gateway] защита от отрицательного остатка: включена (наш выбор, Сейшелы не требуют)"
        );
    } else {
        eprintln!("[gateway] защита от отрицательного остатка ВЫКЛЮЧЕНА — остаток клиента может остаться отрицательным");
    }
    let rearm_band_pct = std::env::var("MARGIN_CALL_REARM_BAND_PERCENT")
        .ok()
        .and_then(|s| s.trim().parse::<i128>().ok())
        .filter(|p| *p >= 0)
        .unwrap_or(10);
    match margin_call_pct {
        Some(p) => println!("[gateway] предупреждение о марже: на уровне {p}%"),
        None => eprintln!(
            "[gateway] ВНИМАНИЕ: MARGIN_CALL_LEVEL_PERCENT не задан — предупреждения о марже НЕТ. Закрытие по марже без предупреждения — срабатывание без шанса отреагировать."
        ),
    }
    if let (Some(call), Some(stop)) = (margin_call_pct, stop_out_pct) {
        if call <= stop {
            eprintln!(
                "[gateway] ВНИМАНИЕ: уровень предупреждения {call}% не выше порога закрытия {stop}% — предупредить раньше закрытия невозможно."
            );
        }
    }
    match stop_out_pct {
        Some(p) => println!("[gateway] принудительное закрытие по марже: ниже {p}%"),
        None => eprintln!(
            "[gateway] ВНИМАНИЕ: MARGIN_STOP_OUT_LEVEL_PERCENT не задан — принудительного закрытия по марже НЕТ. Счёт может уйти в минус без ограничения сверху. Порог задаёт юрисдикция (CMS-24), выдумывать его здесь нельзя."
        ),
    }
    let leverage = leverage_from_env(std::env::var("PLATFORM_LEVERAGE").ok().as_deref());
    println!("[gateway] плечо площадки: 1:{leverage}");
    let internal_money_token =
        money::token_from_env(std::env::var("INTERNAL_MONEY_TOKEN").ok().as_deref());
    let env_flag = |name: &str| {
        matches!(
            std::env::var(name).ok().as_deref().map(str::trim),
            Some("1") | Some("true")
        )
    };
    let real_trading_enabled = env_flag("REAL_TRADING_ENABLED");
    let sltp_edit_enabled =
        levels::enabled_from_env(std::env::var("SLTP_EDIT_ENABLED").ok().as_deref());
    if sltp_edit_enabled {
        println!(
            "[gateway] изменение SL/TP открытых позиций и ордеров: ВКЛЮЧЕНО (SLTP_EDIT_ENABLED=1)"
        );
    } else {
        println!("[gateway] изменение SL/TP: выключено (SLTP_EDIT_ENABLED не 1) — ручки отвечают 409 sltp_edit_disabled; записи этого вида при этом читаются");
    }
    let bus_real_events = env_flag("BUS_REAL_ACCOUNT_EVENTS");
    let leverage_classes_enabled = env_flag("LEVERAGE_CLASSES_ENABLED");
    if leverage_classes_enabled {
        println!(
            "[gateway] плечо по категориям инструментов: ВКЛЮЧЕНО (LEVERAGE_CLASSES_ENABLED=1)"
        );
    } else {
        println!("[gateway] плечо по категориям: выключено (LEVERAGE_CLASSES_ENABLED не 1) — одно плечо счёта на все инструменты; запись leverage_classes_set при этом читается");
    }
    if real_trading_enabled {
        println!("[gateway] торговля на реальных счетах: РАЗРЕШЕНА (проверку личности обеспечивает кабинет)");
    } else {
        eprintln!("[gateway] торговля на реальных счетах закрыта (REAL_TRADING_ENABLED не задан): открытие позиций на real — 403 identity_not_verified");
    }
    if !bus_real_events {
        eprintln!("[gateway] события реальных счетов на шину НЕ публикуются (BUS_REAL_ACCOUNT_EVENTS не задан): потребитель ещё не различает accountKind");
    }
    let catalog = instruments::Catalog::legacy();
    let tenant_instruments = TenantSource::from_env(&catalog.index());
    let catalog = Arc::new(std::sync::RwLock::new(catalog));
    AppState {
        events_tx,
        catalog: catalog.clone(),
        markets: Arc::new(std::sync::Mutex::new(HashMap::new())),
        platform: Arc::new(instruments::Platform::from_env()),
        v2_last_msg: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        v2_daily: Arc::new(std::sync::Mutex::new(HashMap::new())),
        tickers: Arc::new(Mutex::new(HashMap::new())),
        feed_last_msg: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        feed_gap_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        klines_cache: Arc::new(Mutex::new(HashMap::new())),
        depth_cache: Arc::new(Mutex::new(HashMap::new())),
        stats_cache: Arc::new(Mutex::new(HashMap::new())),
        tenant_instruments: Arc::new(tenant_instruments),
        broker: Arc::new(Mutex::new(
            Broker::new(start_cents, leverage)
                .with_instrument_classes(instrument_classes(catalog))
                .with_stop_out_pct(stop_out_pct)
                .with_margin_call_pct(margin_call_pct)
                .with_margin_call_rearm_band_pct(rearm_band_pct)
                .with_negative_balance_protection(protect_negative),
        )),
        store,
        acct_locks: Arc::new(Mutex::new(HashMap::new())),
        quarantined: Arc::new(Mutex::new(std::collections::HashSet::new())),
        journals: Arc::new(Mutex::new(HashMap::new())),
        sso,
        sso_enabled,
        identities: Arc::new(Mutex::new(HashMap::new())),
        bus_enabled: false,
        internal_money_token,
        money_lock: Arc::new(Mutex::new(())),
        real_trading_enabled,
        sltp_edit_enabled,
        leverage_classes_enabled,
        bus_real_events,
        stops: stops::from_env(),
    }
}

fn instrument_classes(
    catalog: Arc<std::sync::RwLock<instruments::Catalog>>,
) -> broker::InstrumentClassOf {
    Arc::new(move |id| {
        catalog
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .and_then(|f| broker::LevClass::parse(&f.category))
    })
}

fn classes_dto(classes: &broker::ClassLeverage) -> std::collections::BTreeMap<&'static str, i128> {
    classes.iter().map(|(c, l)| (c.tag(), *l)).collect()
}

fn leverage_from_env(raw: Option<&str>) -> i128 {
    const DEFAULT: i128 = 1;
    const MAX: i128 = 1000;
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => DEFAULT,
        Some(s) => match s.parse::<i128>() {
            Ok(l) if (1..=MAX).contains(&l) => l,
            _ => {
                eprintln!(
                    "[gateway] PLATFORM_LEVERAGE=«{s}» не целое от 1 до {MAX} — взято плечо {DEFAULT}"
                );
                DEFAULT
            }
        },
    }
}

pub fn announce_account_mode(st: &AppState) {
    if st.sso_enabled {
        println!("[gateway] личность: по сессии SSO, счёт у каждого свой");
        return;
    }
    eprintln!(
        "[gateway] ВНИМАНИЕ: SSO выключен — ВСЕ запросы идут на ОДИН общий счёт #{}.",
        DEFAULT_USER.0
    );
    eprintln!("[gateway] Для автономного терминала это норма. Если этот экземпляр встроен в кабинет — это дефект: люди увидят сделки друг друга, а события на шину не уйдут вовсе (у общего счёта нет внешней личности). Включается SSO_DISABLED=0 при заданном CABINET_URL.");
}

pub fn spawn_bus_relay(st: &mut AppState) {
    match bus::publisher_from_env() {
        Some(publisher) => {
            st.bus_enabled = true;
            bus::spawn_relay(st.store.clone(), publisher);
        }
        None => {
            st.bus_enabled = false;
            eprintln!(
                "[bus] BUS_URL не задан — публикации НЕТ. События копятся в исходящей очереди и уедут после настройки; их число видно в /health (outbox_pending)."
            );
        }
    }
}

pub async fn restore_state(st: &AppState) -> Result<(), persistence::StoreError> {
    st.store.init().await?;
    instruments::load_registry(st).await?;
    let loaded = st.store.load_all().await?;
    st.stops.load(st.store.load_site_min_stops().await?).await;
    let accounts_n = loaded.accounts.len();
    let start_balance = st.broker.lock().await.start_balance();
    {
        let mut b = st.broker.lock().await;
        for (id, snap) in loaded.accounts {
            b.restore(id, snap);
        }
    }
    {
        let mut js = st.journals.lock().await;
        let mut q = st.quarantined.lock().await;
        for (user, events) in loaded.journals {
            match Journal::load(start_balance, events) {
                Ok(j) => {
                    js.insert(user, j);
                }
                Err(fault) => {
                    eprintln!(
                        "[gateway] ЖУРНАЛ ИСПОРЧЕН, счёт {} в карантине: {fault}",
                        user.0
                    );
                    q.insert(user);
                }
            }
        }
    }
    {
        let b = st.broker.lock().await;
        let js = st.journals.lock().await;
        let mut q = st.quarantined.lock().await;
        for user in b.users() {
            let snap = b.snapshot(user);
            let violations = broker::check_snapshot(&snap);
            if !violations.is_empty() {
                q.insert(user);
                for v in &violations {
                    eprintln!(
                        "[gateway] ИНВАРИАНТ НАРУШЕН, счёт {} в карантине: {v}",
                        user.0
                    );
                }
            }
            if q.contains(&user) {
                continue;
            }
            let empty = Journal::new(b.start_balance());
            let status = journal::audit(js.get(&user).unwrap_or(&empty), &snap);
            if !status.ok() {
                q.insert(user);
                eprintln!(
                    "[gateway] ЖУРНАЛ НЕ СХОДИТСЯ [{}], счёт {} в карантине: {status}",
                    status.tag(),
                    user.0
                );
            }
        }
        if !q.is_empty() {
            eprintln!("[gateway] счетов в карантине: {} — мутации по ним отклоняются, состояние не чинится автоматически", q.len());
        }
    }
    if accounts_n > 0 {
        println!("[gateway] восстановлено из хранилища: счетов {accounts_n}");
    }
    instruments::recompute_subscriptions(st).await;
    Ok(())
}

async fn identity_for_bus(st: &AppState, user: UserId) -> Option<BusIdentity> {
    if let Some(found) = st.identities.lock().await.get(&user) {
        return found.clone();
    }
    let read = async {
        let Some((tenant, sub)) = st.store.identity_of(user).await? else {
            return Ok(None);
        };
        let kind = st.store.account_kind_of(user).await?;
        Ok::<_, persistence::StoreError>(Some(BusIdentity { tenant, sub, kind }))
    };
    match read.await {
        Ok(found) => {
            st.identities.lock().await.insert(user, found.clone());
            found
        }
        Err(e) => {
            eprintln!(
                "[bus] личность счёта {} не прочитана, событие НЕ будет собрано: {e}",
                user.0
            );
            None
        }
    }
}

async fn bus_envelopes(
    st: &AppState,
    user: UserId,
    before: bus::Before,
    snap: &broker::AccountSnapshot,
    kinds: &[broker::journal::EventKind],
    sealed: &[broker::journal::AccountEvent],
) -> Vec<persistence::OutboxRecord> {
    if kinds.is_empty() {
        return Vec::new();
    }
    let Some(id) = identity_for_bus(st, user).await else {
        return Vec::new();
    };
    if id.kind == AccountKind::Real && !st.bus_real_events {
        return Vec::new();
    }
    let Some(last) = sealed.last() else {
        return Vec::new();
    };
    let account_id = bus::account_uuid(&id.tenant, &id.sub, id.kind);
    let who = bus::Addressee {
        tenant: &id.tenant,
        sub: &id.sub,
        user: user.0,
        seq: last.seq(),
        at_ms: last.at_ms(),
        kind: id.kind,
        account_id: &account_id,
    };
    let by_id = |id: u32| feed_by_id(st, id);
    bus::envelopes_for(&who, &before, snap, kinds, &by_id)
        .iter()
        .map(|e| e.to_record())
        .collect()
}

async fn account_lock(st: &AppState, user: UserId) -> Arc<Mutex<()>> {
    st.acct_locks
        .lock()
        .await
        .entry(user)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn demo_start_from_env(raw: Option<&str>) -> i128 {
    const DEFAULT: i128 = 10_000_000;
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => DEFAULT,
        Some(s) => match s.parse::<i128>() {
            Ok(c) if c >= 0 => c,
            _ => {
                eprintln!(
                    "[gateway] DEMO_START_BALANCE_CENTS=«{s}» не целое неотрицательное — взято умолчание {DEFAULT}"
                );
                DEFAULT
            }
        },
    }
}

fn stops_err(e: stops::TooClose, price_decimals: u8) -> ApiErr {
    let scale = 10f64.powi(i32::from(price_decimals));
    let fmt = |v: i64| format!("{:.*}", usize::from(price_decimals), v as f64 / scale);
    let what = if e.code == "stop_loss_too_close" {
        "stop loss"
    } else {
        "take profit"
    };
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(ErrResp {
            error: e.code.to_string(),
            extra: Some(serde_json::json!({
                "detail": format!(
                    "{what} is closer than the minimum distance of {} bps ({}) from {}; nearest allowed level is {}",
                    e.min_stop_bps, fmt(e.min_distance), fmt(e.reference), fmt(e.limit)
                ),
                "min_stop_bps": e.min_stop_bps,
                "min_distance": e.min_distance,
                "limit": e.limit,
                "reference": e.reference,
            })),
        }),
    )
}

fn deal_error(e: broker::BrokerError) -> ApiErr {
    use broker::BrokerError as E;
    match e {
        E::InsufficientMargin => err(StatusCode::PAYMENT_REQUIRED, "insufficient_margin"),
        E::StopLossAlreadyReached => err(StatusCode::CONFLICT, "stop_loss_already_reached"),
        E::TakeProfitAlreadyReached => err(StatusCode::CONFLICT, "take_profit_already_reached"),
        E::InsufficientFunds => err(StatusCode::PAYMENT_REQUIRED, "insufficient_funds"),
        E::UnknownPosition => err(StatusCode::NOT_FOUND, "unknown_position"),
        E::UnknownPending => err(StatusCode::NOT_FOUND, "unknown_pending"),
        E::UnknownAccount => err(StatusCode::NOT_FOUND, "unknown_account"),
        E::NotADemoAccount => err(StatusCode::CONFLICT, "not_a_demo_account"),
        E::AccountNotFlat => err(StatusCode::CONFLICT, "account_not_flat"),
        E::InvalidLeverage => err(StatusCode::BAD_REQUEST, "invalid_leverage"),
        E::InvalidFees => err(StatusCode::BAD_REQUEST, "invalid_fees"),
    }
}

fn profile_for_new_account(user: UserId, kind: AccountKind) -> AccountProfile {
    AccountProfile {
        kind,
        ..AccountProfile::legacy_demo(user)
    }
}

async fn demo_start_for(st: &AppState, user: UserId) -> Result<i128, ApiErr> {
    let site = match st.store.identity_of(user).await {
        Ok(Some((tenant, _))) => tenant,
        Ok(None) => return Ok(st.broker.lock().await.start_balance()),
        Err(e) => {
            eprintln!(
                "[gateway] личность счёта {} не прочитана — счёт НЕ заводится: {e}",
                user.0
            );
            return Err(err(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_store_unavailable",
            ));
        }
    };
    match st.tenant_instruments.demo_start_balance(&site).await {
        DemoStart::Named(c) => Ok(c),
        DemoStart::NoPlatformSource => Ok(st.broker.lock().await.start_balance()),
        DemoStart::NotSet => Err(err(StatusCode::CONFLICT, "demo_start_balance_not_set")),
        DemoStart::Unavailable => Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "demo_start_balance_unavailable",
        )),
    }
}

async fn ensure_account_opened(st: &AppState, user: UserId) -> Result<(), ApiErr> {
    if st.broker.lock().await.has_account(user) {
        return Ok(());
    }

    let kind = st.store.account_kind_of(user).await.map_err(|e| {
        eprintln!(
            "[gateway] род счёта {} не прочитан — счёт НЕ заводится: {e}",
            user.0
        );
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_store_unavailable",
        )
    })?;

    let demo_start = match kind {
        AccountKind::Demo => demo_start_for(st, user).await?,
        AccountKind::Real => 0,
    };

    let (kinds, snap, start_balance) = {
        let mut b = st.broker.lock().await;
        if !b.open_account_with(user, profile_for_new_account(user, kind), demo_start) {
            return Ok(());
        }
        (b.take_events(user), b.snapshot(user), b.start_balance())
    };

    async fn undo(st: &AppState, user: UserId) {
        st.broker.lock().await.forget(user);
    }

    let sealed = {
        let mut js = st.journals.lock().await;
        let j = js
            .entry(user)
            .or_insert_with(|| Journal::new(start_balance));
        let sealed = j.seal(now_ms(), &Origin::System, &kinds);
        let diverged = journal::reconcile(&j.preview(&sealed), &snap);
        if !diverged.is_empty() {
            drop(js);
            undo(st, user).await;
            for d in &diverged {
                eprintln!(
                    "[gateway] ЗАВЕДЕНИЕ СЧЁТА {} НЕ СХОДИТСЯ, счёт не заведён: {d}",
                    user.0
                );
            }
            return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "journal_divergence"));
        }
        sealed
    };

    let outgoing = bus_envelopes(
        st,
        user,
        bus::Before {
            closed: 0,
            balance: snap.balance,
        },
        &snap,
        &kinds,
        &sealed,
    )
    .await;

    if let Err(e) = st.store.save_account(user, &snap, &sealed, &outgoing).await {
        undo(st, user).await;
        eprintln!(
            "[gateway] счёт {} не заведён, запись не удалась: {e}",
            user.0
        );
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable"));
    }

    commit_journal(st, user, sealed).await;
    Ok(())
}

async fn with_account<T>(
    st: &AppState,
    user: UserId,
    origin: Origin,
    f: impl FnOnce(&mut Broker) -> Result<T, ApiErr>,
) -> Result<T, ApiErr> {
    with_account_gated(st, user, origin, Gate::Any, f).await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gate {
    Any,
    NotFrozen,
}

async fn trading_frozen(st: &AppState, user: UserId) -> bool {
    st.journals
        .lock()
        .await
        .get(&user)
        .is_some_and(|j| j.state().trading_frozen)
}

async fn with_account_gated<T>(
    st: &AppState,
    user: UserId,
    origin: Origin,
    gate: Gate,
    f: impl FnOnce(&mut Broker) -> Result<T, ApiErr>,
) -> Result<T, ApiErr> {
    let lock = account_lock(st, user).await;
    let mut attempts = 0u32;
    let (out, before, snap, kinds, start_balance, _guard) = loop {
        if st.quarantined.lock().await.contains(&user) {
            return Err(err(StatusCode::LOCKED, "account_quarantined"));
        }

        let guard = lock.clone().lock_owned().await;

        ensure_account_opened(st, user).await?;

        if gate == Gate::NotFrozen && trading_frozen(st, user).await {
            return Err(err(StatusCode::FORBIDDEN, "account_frozen"));
        }

        {
            let mut b = st.broker.lock().await;
            if b.pending_events(user) == 0 {
                let before = b.snapshot(user);
                let out = f(&mut b);
                let kinds = b.take_events(user);
                let snap = b.snapshot(user);
                break (out, before, snap, kinds, b.start_balance(), guard);
            }
        }
        drop(guard);
        attempts += 1;
        if attempts > 200 {
            eprintln!(
                "[gateway] счёт {}: события монитора не записаны за 1 с — операция отклонена",
                user.0
            );
            return Err(err(StatusCode::SERVICE_UNAVAILABLE, "account_busy"));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    let out = out?;

    let violations = broker::check_snapshot(&snap);
    if !violations.is_empty() {
        st.broker.lock().await.restore(user, before);
        for v in &violations {
            eprintln!(
                "[gateway] ИНВАРИАНТ НАРУШЕН операцией, счёт {} откачен: {v}",
                user.0
            );
        }
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "invariant_violated"));
    }

    let (sealed, before_bus) = {
        let mut js = st.journals.lock().await;
        let j = js
            .entry(user)
            .or_insert_with(|| Journal::new(start_balance));
        let before_bus = bus::Before {
            closed: j.state().closed.len(),
            balance: j.state().balance,
        };
        let sealed = j.seal(now_ms(), &origin, &kinds);
        let would_be = j.preview(&sealed);
        let diverged = journal::reconcile(&would_be, &snap);
        if !diverged.is_empty() {
            drop(js);
            st.broker.lock().await.restore(user, before);
            for d in &diverged {
                eprintln!(
                    "[gateway] ЖУРНАЛ НЕ СХОДИТСЯ, операция по счёту {} откачена: {d}",
                    user.0
                );
            }
            return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "journal_divergence"));
        }
        (sealed, before_bus)
    };

    let outgoing = bus_envelopes(st, user, before_bus, &snap, &kinds, &sealed).await;

    if let Err(e) = st.store.save_account(user, &snap, &sealed, &outgoing).await {
        st.broker.lock().await.restore(user, before);
        eprintln!("[gateway] запись состояния не удалась, счёт откачен: {e}");
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable"));
    }

    commit_journal(st, user, sealed).await;
    Ok(out)
}

async fn commit_journal(st: &AppState, user: UserId, sealed: Vec<broker::journal::AccountEvent>) {
    let mut js = st.journals.lock().await;
    let lev_start = {
        let b = st.broker.lock().await;
        (b.leverage(), b.start_balance())
    };
    let j = js.entry(user).or_insert_with(|| Journal::new(lev_start.1));
    for e in sealed {
        if let Err(fault) = j.commit(e) {
            eprintln!(
                "[gateway] ЖУРНАЛ В ПАМЯТИ РАЗОШЁЛСЯ С ХРАНИЛИЩЕМ, счёт {} в карантине: {fault}",
                user.0
            );
            st.quarantined.lock().await.insert(user);
            return;
        }
    }
}

impl AppState {
    pub(crate) fn catalog_snapshot(&self) -> Vec<feed::FeedInstrument> {
        self.catalog
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .by_id
            .values()
            .cloned()
            .collect()
    }

    pub(crate) fn catalog_get(&self, id: u32) -> Option<feed::FeedInstrument> {
        self.catalog
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    pub(crate) fn symbol_index(&self) -> HashMap<String, u32> {
        self.catalog
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .index()
    }

    pub(crate) fn platform_mode(&self, id: u32) -> instruments::Mode {
        self.catalog
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .platform_mode(id)
    }

    pub(crate) fn subscriptions_v1(
        &self,
    ) -> tokio::sync::watch::Receiver<std::collections::BTreeSet<String>> {
        self.platform.subscriptions_v1()
    }

    pub(crate) fn mark_v2_alive(&self) {
        self.v2_last_msg
            .store(now_secs(), std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn v2_silent_for_secs(&self) -> i64 {
        now_secs() - self.v2_last_msg.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn v2_daily_of(&self, id: u32) -> Option<(i64, i64, i64)> {
        self.v2_daily
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .copied()
    }

    pub(crate) fn set_v2_daily(&self, id: u32, v: (i64, i64, i64)) {
        self.v2_daily
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, v);
    }

    pub(crate) fn market_of(&self, id: u32) -> Option<instruments::MarketState> {
        self.markets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned()
    }

    pub(crate) fn mark_feed_alive(&self) {
        self.feed_last_msg
            .store(now_secs(), std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn note_feed_gap(&self) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        let last = self.feed_last_msg.load(Relaxed);
        let stale = last == 0 || now_secs() - last > FEED_STALE_AFTER_SECS;
        if stale {
            self.feed_gap_pending.store(true, Relaxed);
            return false;
        }
        self.feed_gap_pending.swap(false, Relaxed)
    }
}

fn normalize_symbol(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

fn symbol_index(instruments: &[feed::FeedInstrument]) -> HashMap<String, u32> {
    let mut out: HashMap<String, u32> = HashMap::new();
    for f in instruments.iter().filter(|f| f.source == feed::Source::V1) {
        let binance = normalize_symbol(&f.binance);
        let mut spellings = vec![binance.clone(), normalize_symbol(&f.symbol)];
        if let Some(base) = binance.strip_suffix("USDT") {
            spellings.push(format!("{base}USD"));
        }
        for s in spellings {
            match out.insert(s.clone(), f.id) {
                Some(other) if other != f.id => {
                    eprintln!(
                        "[gateway] КОЛЛИЗИЯ СИМВОЛОВ: «{s}» указывает и на {other}, и на {}; \
                         разрешения по этому символу недостоверны",
                        f.id
                    );
                }
                _ => {}
            }
        }
    }
    for f in instruments.iter().filter(|f| !f.canonical.is_empty()) {
        let canonical = normalize_symbol(&f.canonical);
        let mut spellings = vec![canonical];
        if f.source == feed::Source::V2 {
            spellings.push(normalize_symbol(&f.binance));
        }
        for s in spellings {
            if let Some(other) = out.insert(s.clone(), f.id) {
                if other != f.id {
                    eprintln!(
                        "[gateway] символ платформы «{s}» отдан инструменту {} (до этого псевдоним инструмента {other})",
                        f.id
                    );
                }
            }
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TenantAllowList {
    Resolved(Vec<u32>),
    Unknown(Vec<String>),
}

impl TenantAllowList {
    fn resolve(index: &HashMap<String, u32>, symbols: &[String]) -> Self {
        let mut ids = Vec::with_capacity(symbols.len());
        let mut unknown = Vec::new();
        for s in symbols {
            match index.get(&normalize_symbol(s)) {
                Some(id) => ids.push(*id),
                None => unknown.push(s.clone()),
            }
        }
        if unknown.is_empty() {
            ids.sort_unstable();
            ids.dedup();
            TenantAllowList::Resolved(ids)
        } else {
            TenantAllowList::Unknown(unknown)
        }
    }
}

enum NoList {
    Unavailable,
    Unreadable,
}

impl NoList {
    fn api(&self) -> ApiErr {
        match self {
            NoList::Unavailable => err(
                StatusCode::SERVICE_UNAVAILABLE,
                "tenant_instruments_unavailable",
            ),
            NoList::Unreadable => err(
                StatusCode::SERVICE_UNAVAILABLE,
                "tenant_instruments_unreadable",
            ),
        }
    }
}

enum TenantSource {
    Cms {
        base: String,
        api_key: String,
        http: reqwest::Client,
    },
    Static(HashMap<String, TenantAllowList>),
    None,
}

impl TenantSource {
    fn from_env(index: &HashMap<String, u32>) -> Self {
        let base = std::env::var("CMS_BASE_URL")
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let api_key = std::env::var("CMS_API_KEY").unwrap_or_default();
        if !base.is_empty() && !api_key.is_empty() {
            println!("[gateway] allow-list: источник — CMS {base}/cms/sites/{{slug}} (сервер-сервер, X-API-Key)");
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap_or_default();
            return TenantSource::Cms {
                base,
                api_key,
                http,
            };
        }
        if !base.is_empty() || !api_key.is_empty() {
            eprintln!("[gateway] allow-list: CMS сконфигурирована наполовину (нужны и CMS_BASE_URL, и CMS_API_KEY) — источник CMS не используется");
        }
        match parse_tenant_instruments(std::env::var("TENANT_INSTRUMENTS").ok().as_deref(), index) {
            Some(map) => {
                println!(
                    "[gateway] allow-list: источник — TENANT_INSTRUMENTS, сайтов {}",
                    map.len()
                );
                TenantSource::Static(map)
            }
            None => {
                println!("[gateway] allow-list: источника нет — в тенантном режиме торговля закрыта (fail-closed)");
                TenantSource::None
            }
        }
    }

    async fn demo_start_balance(&self, site: &str) -> DemoStart {
        let (base, api_key, http) = match self {
            TenantSource::Cms {
                base,
                api_key,
                http,
            } => (base, api_key, http),
            _ => return DemoStart::NoPlatformSource,
        };
        let url = format!("{base}/cms/sites/{site}");
        let resp = match http.get(&url).header("X-API-Key", api_key).send().await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[gateway] {site}: CMS недоступна ({e}) — счёт не заводится");
                return DemoStart::Unavailable;
            }
        };
        if !resp.status().is_success() {
            eprintln!(
                "[gateway] {site}: CMS ответила {} — счёт не заводится",
                resp.status()
            );
            return DemoStart::Unavailable;
        }
        let body: SiteConfigDto = match resp.json().await {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[gateway] {site}: ответ CMS не разобран ({e}) — счёт не заводится");
                return DemoStart::Unavailable;
            }
        };
        match body.demo_start_balance_cents {
            Some(c) if c >= 0 => DemoStart::Named(c),
            Some(c) => {
                eprintln!(
                    "[gateway] {site}: стартовая сумма демо-счёта отрицательная ({c}) — счёт не заводится"
                );
                DemoStart::NotSet
            }
            None => {
                eprintln!(
                    "[gateway] {site}: в ответе CMS нет demoStartBalanceCents — сумма не названа, счёт не заводится"
                );
                DemoStart::NotSet
            }
        }
    }

    async fn site_lists(
        &self,
        site: &str,
        index: &HashMap<String, u32>,
    ) -> Result<(Vec<u32>, Vec<u32>), NoList> {
        let mut close_only: Vec<u32> = Vec::new();
        let list = match self {
            TenantSource::None => {
                eprintln!(
                    "[gateway] {site}: источник allow-list не сконфигурирован — торговля закрыта"
                );
                return Err(NoList::Unavailable);
            }
            TenantSource::Static(map) => match map.get(site) {
                Some(l) => l.clone(),
                None => {
                    eprintln!("[gateway] {site}: сайта нет в allow-list — торговля закрыта");
                    return Err(NoList::Unavailable);
                }
            },
            TenantSource::Cms {
                base,
                api_key,
                http,
            } => {
                let url = format!("{base}/cms/sites/{site}");
                let resp = http.get(&url).header("X-API-Key", api_key).send().await;
                let resp = match resp {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("[gateway] {site}: CMS недоступна ({e}) — торговля закрыта");
                        return Err(NoList::Unavailable);
                    }
                };
                if !resp.status().is_success() {
                    eprintln!(
                        "[gateway] {site}: CMS ответила {} — торговля закрыта",
                        resp.status()
                    );
                    return Err(NoList::Unavailable);
                }
                let body: SiteConfigDto = match resp.json().await {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!(
                            "[gateway] {site}: ответ CMS не разобран ({e}) — торговля закрыта"
                        );
                        return Err(NoList::Unavailable);
                    }
                };
                close_only = body
                    .close_only_instruments
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|s| index.get(&normalize_symbol(s)).copied())
                    .collect();
                match body.instruments {
                    None => {
                        eprintln!("[gateway] {site}: в ответе CMS нет поля instruments — список неизвестен, торговля закрыта");
                        return Err(NoList::Unavailable);
                    }
                    Some(symbols) => TenantAllowList::resolve(index, &symbols),
                }
            }
        };
        match list {
            TenantAllowList::Resolved(ids) => {
                close_only.retain(|id| ids.contains(id));
                Ok((ids, close_only))
            }
            TenantAllowList::Unknown(unknown) => {
                eprintln!(
                    "[gateway] {site}: allow-list содержит неизвестные терминалу символы [{}] — \
                     список не принят целиком, торговля закрыта. Молча выбросить их значило бы \
                     тихо сузить права (Р-024)",
                    unknown.join(", ")
                );
                Err(NoList::Unreadable)
            }
        }
    }
}

#[derive(Deserialize)]
struct SiteConfigDto {
    #[serde(default)]
    instruments: Option<Vec<String>>,
    #[serde(default, rename = "demoStartBalanceCents")]
    demo_start_balance_cents: Option<i128>,
    #[serde(default, rename = "closeOnlyInstruments")]
    close_only_instruments: Option<Vec<String>>,
}

enum DemoStart {
    Named(i128),
    NotSet,
    Unavailable,
    NoPlatformSource,
}

fn parse_tenant_instruments(
    raw: Option<&str>,
    index: &HashMap<String, u32>,
) -> Option<HashMap<String, TenantAllowList>> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let mut out: HashMap<String, TenantAllowList> = HashMap::new();
    for site_part in raw.split(';').filter(|p| !p.trim().is_empty()) {
        let (site, symbols) = site_part.split_once(':').unwrap_or((site_part, ""));
        let symbols: Vec<String> = symbols
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        out.insert(
            site.trim().to_string(),
            TenantAllowList::resolve(index, &symbols),
        );
    }
    Some(out)
}

struct Principal {
    user: UserId,
    tenant: Option<String>,
}

async fn ensure_can_open(st: &AppState, who: &Principal, instrument: u32) -> Result<(), ApiErr> {
    if st.broker.lock().await.snapshot(who.user).profile.kind == AccountKind::Real
        && !st.real_trading_enabled
    {
        return Err(err(StatusCode::FORBIDDEN, "identity_not_verified"));
    }

    let platform_close_only = st.catalog_get(instrument).is_some()
        && st.platform_mode(instrument) == instruments::Mode::CloseOnly;
    let Some(tenant) = who.tenant.as_deref() else {
        if platform_close_only {
            return Err(err(StatusCode::CONFLICT, "instrument_close_only"));
        }
        return Ok(());
    };
    let (allowed, close_only) = st
        .tenant_instruments
        .site_lists(tenant, &st.symbol_index())
        .await
        .map_err(|e| e.api())?;
    st.platform
        .note_site_close_only(tenant, close_only.iter().copied().collect());
    if !allowed.contains(&instrument) {
        return Err(err(StatusCode::FORBIDDEN, "instrument_not_allowed"));
    }
    if platform_close_only || close_only.contains(&instrument) {
        return Err(err(StatusCode::CONFLICT, "instrument_close_only"));
    }
    Ok(())
}

fn market_closed(code: &str, next_open: Option<i64>) -> ApiErr {
    (
        StatusCode::CONFLICT,
        Json(ErrResp {
            error: code.to_string(),
            extra: Some(serde_json::json!({ "next_open": next_open })),
        }),
    )
}

fn market_gate(st: &AppState, instrument: u32) -> Result<(), ApiErr> {
    let Some(m) = st.market_of(instrument) else {
        return Ok(());
    };
    match m.status {
        instruments::Status::Closed => Err(market_closed("market_closed", m.next_open)),
        instruments::Status::Halted => Err(market_closed("market_halted", m.next_open)),
        instruments::Status::Open if !m.tradable(now_ms()) => {
            Err(err(StatusCode::SERVICE_UNAVAILABLE, "no price yet"))
        }
        instruments::Status::Open => Ok(()),
    }
}

fn qty_gate(fi: &feed::FeedInstrument, qty: i64) -> Result<(), ApiErr> {
    if fi.legacy {
        return Ok(());
    }
    if let Some(min) = fi.min_qty.filter(|m| *m > 0) {
        if qty < min {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ErrResp {
                    error: "qty_below_min".into(),
                    extra: Some(serde_json::json!({ "min_qty": min })),
                }),
            ));
        }
    }
    if let Some(step) = fi.qty_step.filter(|s| *s > 0) {
        if qty % step != 0 {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ErrResp {
                    error: "qty_not_multiple_of_step".into(),
                    extra: Some(serde_json::json!({ "qty_step": step })),
                }),
            ));
        }
    }
    Ok(())
}

async fn marks(st: &AppState) -> HashMap<u32, i64> {
    st.tickers
        .lock()
        .await
        .iter()
        .map(|(id, t)| (*id, t.last))
        .collect()
}

fn feed_by_id(st: &AppState, id: u32) -> Option<feed::FeedInstrument> {
    st.catalog_get(id)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn require_principal(st: &AppState, headers: &HeaderMap) -> Result<Principal, ApiErr> {
    if !st.sso_enabled {
        return Ok(Principal {
            user: DEFAULT_USER,
            tenant: None,
        });
    }
    let token = bearer(headers).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing session"))?;
    let (user, tenant) = st
        .sso
        .resolve(token)
        .await
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "invalid session"))?;
    Ok(Principal {
        user,
        tenant: Some(tenant),
    })
}

async fn require_user(st: &AppState, headers: &HeaderMap) -> Result<UserId, ApiErr> {
    Ok(require_principal(st, headers).await?.user)
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/instruments", get(list_instruments))
        .route("/candles/:instrument", get(get_candles))
        .route("/depth/:instrument", get(get_depth))
        .route("/stats/:instrument", get(get_stats))
        .route("/deals", post(open_deal).get(list_deals))
        .route("/deals/closed", get(list_closed))
        .route("/deals/:id/close", post(close_deal))
        .route("/pending", post(place_pending).get(list_pending))
        .route("/pending/:id", delete(cancel_pending))
        .route("/deals/:id/levels", put(levels::edit_deal_levels))
        .route("/pending/:id/levels", put(levels::edit_pending_levels))
        .route("/account", get(account))
        .route("/account/journal", get(account_journal))
        .route("/account/demo-reset", post(demo_reset))
        .route("/stream", get(ws_stream))
        .route("/v1/session", post(session_create))
        .route("/v1/session/logout", post(session_logout))
        .merge(money::routes())
        .merge(stops::routes())
        .merge(instruments::routes())
        .with_state(state)
}

#[derive(Serialize, Debug)]
struct ErrResp {
    error: String,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    extra: Option<serde_json::Value>,
}
type ApiErr = (StatusCode, Json<ErrResp>);
fn err(code: StatusCode, msg: &str) -> ApiErr {
    (
        code,
        Json(ErrResp {
            error: msg.to_string(),
            extra: None,
        }),
    )
}

#[derive(Deserialize)]
struct CandleQuery {
    tf: Option<u32>,
    limit: Option<usize>,
    end: Option<i64>,
}
#[derive(Serialize)]
struct CandleDto {
    time: i64,
    open: i64,
    high: i64,
    low: i64,
    close: i64,
    volume: Option<i64>,
}
#[derive(Deserialize)]
struct DealReq {
    instrument: u32,
    side: String,
    qty: i64,
    sl: Option<i64>,
    tp: Option<i64>,
}
#[derive(Serialize)]
struct DealDto {
    id: u64,
    instrument: u32,
    symbol: String,
    side: String,
    qty: i64,
    entry: i64,
    mark: i64,
    pnl: i128,
    sl: Option<i64>,
    tp: Option<i64>,
    price_decimals: u8,
    qty_decimals: u8,
}
#[derive(Deserialize)]
struct PendingReq {
    instrument: u32,
    side: String,
    qty: i64,
    price: i64,
    sl: Option<i64>,
    tp: Option<i64>,
}
#[derive(Serialize)]
struct PendingDto {
    id: u64,
    instrument: u32,
    symbol: String,
    side: String,
    qty: i64,
    price: i64,
    sl: Option<i64>,
    tp: Option<i64>,
    price_decimals: u8,
    qty_decimals: u8,
}
#[derive(Serialize)]
struct ClosedDto {
    instrument: u32,
    symbol: String,
    side: String,
    qty: i64,
    entry: i64,
    exit: i64,
    pnl: i128,
    price_decimals: u8,
    qty_decimals: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    across_gap: Option<bool>,
    commission: i128,
    #[serde(skip_serializing_if = "Option::is_none")]
    market_exit: Option<i64>,
}
#[derive(Serialize)]
struct AccountDto {
    opened: bool,
    model: Option<&'static str>,
    balance: Option<i128>,
    equity: Option<i128>,
    used_margin: Option<i128>,
    free_margin: Option<i128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    margin_level_pct: Option<i128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_out_level_pct: Option<i128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    margin_call_level_pct: Option<i128>,
    negative_balance_protection: bool,
    open_pnl: Option<i128>,
    leverage: i128,
    leverage_classes: std::collections::BTreeMap<&'static str, i128>,
    markup_bps: i64,
    commission_bps: i64,
    trading_frozen: bool,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    min_stop_bps: i64,
    sltp_edit: bool,
}
#[derive(Serialize)]
struct InstrumentDto {
    id: u32,
    symbol: String,
    base: u32,
    quote: u32,
    price_decimals: u8,
    qty_decimals: u8,
    last: Option<i64>,
    bid: Option<i64>,
    ask: Option<i64>,
    high: Option<i64>,
    change: Option<f64>,
    low: Option<i64>,
    volume: Option<i64>,
    quote_volume: Option<i64>,
    canonical: String,
    name: Option<String>,
    category: String,
    tick_size: Option<i64>,
    qty_step: Option<i64>,
    min_qty: Option<i64>,
    contract_size: Option<String>,
    lot_decimals: Option<u8>,
    has_depth: bool,
    has_trades: bool,
    has_1s: bool,
    mode: &'static str,
    market: MarketDto,
    session: Option<serde_json::Value>,
}
#[derive(Serialize)]
struct MarketDto {
    status: &'static str,
    next_open: Option<i64>,
    reason: Option<String>,
    tradable: bool,
}
#[derive(Serialize)]
struct DepthResp {
    price_decimals: u8,
    qty_decimals: u8,
    bids: Vec<(i64, i64)>,
    asks: Vec<(i64, i64)>,
}
#[derive(Deserialize)]
struct DepthQuery {
    limit: Option<usize>,
}
#[derive(Deserialize)]
struct SiteQuery {
    site: Option<String>,
}
#[derive(Serialize, Clone)]
struct StatDto {
    tf: u32,
    change: Option<f64>,
}

#[derive(Serialize)]
struct HealthDto {
    status: &'static str,
    storage: &'static str,
    durable: bool,
    sso: bool,
    feed_fresh: bool,
    feed_age_secs: Option<i64>,
    priced_instruments: usize,
    bus: bool,
    outbox_pending: Option<i64>,
    shared_account: bool,
    internal_money: bool,
    feed_v2: Option<bool>,
    feed_v2_age_secs: Option<i64>,
    platform_instruments_source: &'static str,
    platform_instruments_version: Option<i64>,
    platform_instruments: usize,
}

const FEED_STALE_AFTER_SECS: i64 = 30;

async fn health(State(st): State<AppState>) -> Json<HealthDto> {
    let last = st.feed_last_msg.load(std::sync::atomic::Ordering::Relaxed);
    let age = (last > 0).then(|| now_secs() - last);
    let (cat_source, cat_version, cat_len) = {
        let cat = st.catalog.read().unwrap_or_else(|e| e.into_inner());
        (cat.source, cat.version, cat.platform.len())
    };
    Json(HealthDto {
        status: "ok",
        storage: st.store.kind(),
        durable: st.store.durable(),
        sso: st.sso_enabled,
        feed_fresh: age.is_some_and(|a| a <= FEED_STALE_AFTER_SECS),
        feed_age_secs: age,
        priced_instruments: st.tickers.lock().await.len(),
        bus: st.bus_enabled,
        outbox_pending: st.store.outbox_backlog().await.ok(),
        shared_account: !st.sso_enabled,
        internal_money: st.internal_money_token.is_some(),
        feed_v2: st
            .platform
            .v2
            .as_ref()
            .map(|_| st.v2_silent_for_secs() <= instruments::QUOTE_STALE_MS / 1000),
        feed_v2_age_secs: st.platform.v2.as_ref().and_then(|_| {
            let last = st.v2_last_msg.load(std::sync::atomic::Ordering::Relaxed);
            (last > 0).then(|| now_secs() - last)
        }),
        platform_instruments_source: cat_source,
        platform_instruments_version: cat_version,
        platform_instruments: cat_len,
    })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn list_instruments(
    State(st): State<AppState>,
    Query(q): Query<SiteQuery>,
) -> Result<Json<Vec<InstrumentDto>>, ApiErr> {
    let mut site_close_only: Vec<u32> = Vec::new();
    let allowed: Option<Vec<u32>> = match q.site.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(site) => {
            let (list, close_only) = st
                .tenant_instruments
                .site_lists(site, &st.symbol_index())
                .await
                .map_err(|e| e.api())?;
            st.platform
                .note_site_close_only(site, close_only.iter().copied().collect());
            site_close_only = close_only;
            if list.is_empty() {
                eprintln!(
                    "[gateway] {site}: allow-list пуст — сайту не разрешён ни один инструмент"
                );
                return Err(err(StatusCode::FORBIDDEN, "no_instruments_allowed"));
            }
            Some(list)
        }
    };
    let exposed = instruments::exposure_ids(&st).await;
    let (shown, modes) = {
        let cat = st.catalog.read().unwrap_or_else(|e| e.into_inner());
        let shown = cat.shown(&exposed);
        let modes: HashMap<u32, instruments::Mode> = shown
            .iter()
            .map(|f| (f.id, cat.platform_mode(f.id)))
            .collect();
        (shown, modes)
    };
    let now = now_ms();
    let markets = st.markets.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let tickers = st.tickers.lock().await;
    let out = shown
        .iter()
        .filter(|f| allowed.as_ref().is_none_or(|a| a.contains(&f.id)))
        .map(|f| {
            let t = tickers.get(&f.id);
            let close_only = modes.get(&f.id) == Some(&instruments::Mode::CloseOnly)
                || site_close_only.contains(&f.id);
            let market = match markets.get(&f.id) {
                Some(m) => MarketDto {
                    status: m.status.tag(),
                    next_open: m.next_open,
                    reason: m.reason.clone(),
                    tradable: m.tradable(now),
                },
                None => MarketDto {
                    status: "open",
                    next_open: None,
                    reason: None,
                    tradable: true,
                },
            };
            InstrumentDto {
                id: f.id,
                symbol: f.symbol.clone(),
                base: f.base,
                quote: f.quote,
                price_decimals: f.price_decimals,
                qty_decimals: f.qty_decimals,
                last: t.map(|t| t.last),
                bid: t.map(|t| t.bid),
                ask: t.map(|t| t.ask),
                high: t.map(|t| t.high),
                change: t.map(|t| t.change).filter(|c| c.is_finite()),
                low: t.and_then(|t| t.low),
                volume: t.and_then(|t| t.volume),
                quote_volume: t.and_then(|t| t.quote_volume),
                canonical: f.canonical.clone(),
                name: f.name.clone(),
                category: f.category.clone(),
                tick_size: f.tick_size,
                qty_step: f.qty_step,
                min_qty: f.min_qty,
                contract_size: f.contract_size.clone(),
                lot_decimals: f.lot_decimals,
                has_depth: f.has_depth,
                has_trades: f.has_trades,
                has_1s: f.has_1s,
                mode: if close_only {
                    instruments::Mode::CloseOnly.tag()
                } else {
                    instruments::Mode::Trade.tag()
                },
                market,
                session: f.session.clone(),
            }
        })
        .collect();
    Ok(Json(out))
}

async fn get_candles(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Query(q): Query<CandleQuery>,
) -> Result<Json<Vec<CandleDto>>, ApiErr> {
    let tf = q.tf.unwrap_or(3600);
    let limit = q.limit.unwrap_or(300).min(1000);
    let fi = feed_by_id(&st, id).ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    if tf == 1 && !fi.has_1s {
        return Err(err(StatusCode::BAD_REQUEST, "interval_unsupported"));
    }
    let key = (id, tf);
    if q.end.is_none() {
        let cache = st.klines_cache.lock().await;
        if let Some((t, v)) = cache.get(&key) {
            if t.elapsed() < Duration::from_secs(2) {
                return Ok(Json(tail_dto(v, limit)));
            }
        }
    }
    let fetch_n = if q.end.is_some() { limit } else { 500 };
    let fetched = match fi.source {
        feed::Source::V1 => {
            feed::fetch_klines(
                &fi.binance,
                feed::interval(tf),
                fetch_n,
                q.end,
                fi.price_decimals,
                fi.qty_decimals,
            )
            .await
        }
        feed::Source::V2 => match (&st.platform.v2, feed_v2::interval(tf)) {
            (Some(v2), Some(iv)) => {
                feed_v2::fetch_candles(&st.platform.http, v2, &fi, iv, fetch_n, q.end).await
            }
            (None, _) => Err("модуль v2 не настроен".into()),
            (_, None) => return Err(err(StatusCode::BAD_REQUEST, "interval_unsupported")),
        },
    };
    match fetched {
        Ok(v) => {
            let out = tail_dto(&v, limit);
            if q.end.is_none() {
                st.klines_cache
                    .lock()
                    .await
                    .insert(key, (Instant::now(), v));
            }
            Ok(Json(out))
        }
        Err(e) => {
            eprintln!("[gateway] свечи {} tf={tf} недоступны: {e}", fi.symbol);
            Err(err(StatusCode::SERVICE_UNAVAILABLE, "candles unavailable"))
        }
    }
}
fn tail_dto(v: &[feed::Kline], limit: usize) -> Vec<CandleDto> {
    v[v.len().saturating_sub(limit)..]
        .iter()
        .map(|k| CandleDto {
            time: k.time,
            open: k.open,
            high: k.high,
            low: k.low,
            close: k.close,
            volume: k.volume,
        })
        .collect()
}

async fn get_depth(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Query(q): Query<DepthQuery>,
) -> Result<Json<DepthResp>, ApiErr> {
    let fi = feed_by_id(&st, id).ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    if !fi.has_depth {
        return Err(err(StatusCode::NOT_FOUND, "depth_unavailable"));
    }
    let limit = match q.limit.unwrap_or(20) {
        0..=20 => 20,
        21..=50 => 50,
        51..=100 => 100,
        101..=500 => 500,
        _ => 1000,
    };
    {
        let cache = st.depth_cache.lock().await;
        if let Some((t, d)) = cache.get(&(id, limit)) {
            if t.elapsed() < Duration::from_millis(400) {
                return Ok(Json(DepthResp {
                    price_decimals: fi.price_decimals,
                    qty_decimals: fi.qty_decimals,
                    bids: d.bids.clone(),
                    asks: d.asks.clone(),
                }));
            }
        }
    }
    let fetched = match (fi.source, &st.platform.v2) {
        (feed::Source::V1, _) => {
            feed::fetch_depth(&fi.binance, limit, fi.price_decimals, fi.qty_decimals).await
        }
        (feed::Source::V2, Some(v2)) => {
            feed_v2::fetch_depth(&st.platform.http, v2, &fi, limit.min(100)).await
        }
        (feed::Source::V2, None) => Err("модуль v2 не настроен".into()),
    };
    match fetched {
        Ok(d) => {
            let resp = DepthResp {
                price_decimals: fi.price_decimals,
                qty_decimals: fi.qty_decimals,
                bids: d.bids.clone(),
                asks: d.asks.clone(),
            };
            st.depth_cache
                .lock()
                .await
                .insert((id, limit), (Instant::now(), d));
            Ok(Json(resp))
        }
        Err(e) => {
            eprintln!("[gateway] стакан {} недоступен: {e}", fi.symbol);
            Err(err(StatusCode::SERVICE_UNAVAILABLE, "depth unavailable"))
        }
    }
}

async fn get_stats(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Vec<StatDto>>, ApiErr> {
    let fi = feed_by_id(&st, id).ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    {
        let cache = st.stats_cache.lock().await;
        if let Some((t, v)) = cache.get(&id) {
            if t.elapsed() < Duration::from_secs(3) {
                return Ok(Json(v.clone()));
            }
        }
    }
    let futs = STAT_TFS.iter().map(|&tf| {
        let sym = fi.binance.clone();
        let label = fi.symbol.clone();
        let source = fi.source;
        let st = st.clone();
        async move {
            let got = match (source, &st.platform.v2) {
                (feed::Source::V1, _) => feed::fetch_change(&sym, feed::interval(tf)).await,
                (feed::Source::V2, Some(v2)) => {
                    feed_v2::fetch_change(
                        &st.platform.http,
                        v2,
                        &sym,
                        feed_v2::interval(tf).unwrap_or("1h"),
                    )
                    .await
                }
                (feed::Source::V2, None) => Err("модуль v2 не настроен".into()),
            };
            match got {
                Ok(change) => StatDto {
                    tf,
                    change: Some(change),
                },
                Err(e) => {
                    eprintln!("[gateway] статистика {label} tf={tf} недоступна: {e}");
                    StatDto { tf, change: None }
                }
            }
        }
    });
    let out: Vec<StatDto> = futures_util::future::join_all(futs).await;
    st.stats_cache
        .lock()
        .await
        .insert(id, (Instant::now(), out.clone()));
    Ok(Json(out))
}

async fn open_deal(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DealReq>,
) -> Result<Json<DealDto>, ApiErr> {
    let who = require_principal(&st, &headers).await?;
    ensure_can_open(&st, &who, req.instrument).await?;
    let user = who.user;
    let side = match req.side.as_str() {
        "buy" => PosSide::Long,
        "sell" => PosSide::Short,
        _ => return Err(err(StatusCode::BAD_REQUEST, "side must be buy|sell")),
    };
    if req.qty <= 0 {
        return Err(err(StatusCode::BAD_REQUEST, "qty must be > 0"));
    }
    let fi = feed_by_id(&st, req.instrument)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    market_gate(&st, req.instrument)?;
    qty_gate(&fi, req.qty)?;
    let (entry, mark) = {
        let tickers = st.tickers.lock().await;
        let t = tickers
            .get(&req.instrument)
            .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "no price yet"))?;
        (
            if matches!(side, PosSide::Long) {
                t.ask
            } else {
                t.bid
            },
            t.last,
        )
    };
    let min_bps = st.stops.for_tenant(who.tenant.as_deref()).await;
    stops::check(side, entry, req.sl, req.tp, min_bps)
        .map_err(|e| stops_err(e, fi.price_decimals))?;
    let m = marks(&st).await;
    let id = with_account_gated(&st, user, Origin::Client, Gate::NotFrozen, |b| {
        b.open(
            user,
            &m,
            req.instrument,
            side,
            req.qty,
            entry,
            fi.price_decimals,
            fi.qty_decimals,
            req.sl,
            req.tp,
        )
        .map_err(deal_error)
        .map(|id| {
            let exec = b
                .positions(user)
                .into_iter()
                .find(|p| p.id == id)
                .map(|p| p.entry)
                .unwrap_or(entry);
            (id, exec)
        })
    })
    .await?;
    let (id, entry) = id;
    Ok(Json(DealDto {
        id,
        instrument: req.instrument,
        symbol: fi.symbol,
        side: req.side,
        qty: req.qty,
        entry,
        mark,
        pnl: 0,
        sl: req.sl,
        tp: req.tp,
        price_decimals: fi.price_decimals,
        qty_decimals: fi.qty_decimals,
    }))
}

async fn place_pending(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PendingReq>,
) -> Result<Json<PendingDto>, ApiErr> {
    let who = require_principal(&st, &headers).await?;
    ensure_can_open(&st, &who, req.instrument).await?;
    let user = who.user;
    let side = match req.side.as_str() {
        "buy" => PosSide::Long,
        "sell" => PosSide::Short,
        _ => return Err(err(StatusCode::BAD_REQUEST, "side must be buy|sell")),
    };
    if req.qty <= 0 || req.price <= 0 {
        return Err(err(StatusCode::BAD_REQUEST, "qty and price must be > 0"));
    }
    let fi = feed_by_id(&st, req.instrument)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    qty_gate(&fi, req.qty)?;
    let min_bps = st.stops.for_tenant(who.tenant.as_deref()).await;
    stops::check(side, req.price, req.sl, req.tp, min_bps)
        .map_err(|e| stops_err(e, fi.price_decimals))?;
    let id = with_account_gated(&st, user, Origin::Client, Gate::NotFrozen, |b| {
        b.place_pending(
            user,
            req.instrument,
            side,
            req.qty,
            req.price,
            fi.price_decimals,
            fi.qty_decimals,
            req.sl,
            req.tp,
        )
        .map_err(deal_error)
    })
    .await?;
    Ok(Json(PendingDto {
        id,
        instrument: req.instrument,
        symbol: fi.symbol,
        side: req.side,
        qty: req.qty,
        price: req.price,
        sl: req.sl,
        tp: req.tp,
        price_decimals: fi.price_decimals,
        qty_decimals: fi.qty_decimals,
    }))
}

async fn list_pending(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PendingDto>>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    let b = st.broker.lock().await;
    let out = b
        .pendings(user)
        .into_iter()
        .map(|p| PendingDto {
            id: p.id,
            instrument: p.instrument,
            symbol: feed_by_id(&st, p.instrument)
                .map(|f| f.symbol)
                .unwrap_or_default(),
            side: if matches!(p.side, PosSide::Long) {
                "buy"
            } else {
                "sell"
            }
            .to_string(),
            qty: p.qty,
            price: p.price,
            sl: p.sl,
            tp: p.tp,
            price_decimals: p.pd,
            qty_decimals: p.qd,
        })
        .collect();
    Ok(Json(out))
}

async fn cancel_pending(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Result<StatusCode, ApiErr> {
    let user = require_user(&st, &headers).await?;
    with_account(&st, user, Origin::Client, |b| {
        b.cancel_pending(user, id)
            .map_err(|_| err(StatusCode::NOT_FOUND, "unknown pending"))
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_closed(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ClosedDto>>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    let b = st.broker.lock().await;
    let out = b
        .closed_deals(user)
        .into_iter()
        .rev()
        .map(|d| ClosedDto {
            instrument: d.instrument,
            symbol: feed_by_id(&st, d.instrument)
                .map(|f| f.symbol)
                .unwrap_or_default(),
            side: if matches!(d.side, PosSide::Long) {
                "buy"
            } else {
                "sell"
            }
            .to_string(),
            qty: d.qty,
            entry: d.entry,
            exit: d.exit,
            pnl: d.pnl,
            level: d.level,
            across_gap: d.across_gap,
            commission: d.fees.commission,
            market_exit: d.fees.market_exit,
            price_decimals: d.pd,
            qty_decimals: d.qd,
        })
        .collect();
    Ok(Json(out))
}

async fn list_deals(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<DealDto>>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    let m = marks(&st).await;
    let b = st.broker.lock().await;
    let minor = b.snapshot(user).profile.currency.minor_per_unit();
    let out = b
        .positions(user)
        .into_iter()
        .map(|p| {
            let mark = *m.get(&p.instrument).unwrap_or(&p.entry);
            let symbol = feed_by_id(&st, p.instrument)
                .map(|f| f.symbol)
                .unwrap_or_default();
            DealDto {
                id: p.id,
                instrument: p.instrument,
                symbol,
                side: if matches!(p.side, PosSide::Long) {
                    "buy"
                } else {
                    "sell"
                }
                .to_string(),
                qty: p.qty,
                entry: p.entry,
                mark,
                pnl: p.unrealized(mark, minor),
                sl: p.sl,
                tp: p.tp,
                price_decimals: p.pd,
                qty_decimals: p.qd,
            }
        })
        .collect();
    Ok(Json(out))
}

async fn close_deal(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    let inst = st
        .broker
        .lock()
        .await
        .positions(user)
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| p.instrument);
    if let Some(inst) = inst {
        market_gate(&st, inst)?;
    }
    let m = marks(&st).await;
    if inst.is_some_and(|i| !m.contains_key(&i)) {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "no price yet"));
    }
    let (pnl, mark) = with_account(&st, user, Origin::Client, |b| {
        let inst = b
            .positions(user)
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.instrument)
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown position"))?;
        let mark = *m.get(&inst).unwrap_or(&0);
        let pnl = b
            .close(user, id, mark)
            .map_err(|_| err(StatusCode::NOT_FOUND, "unknown position"))?;
        Ok((pnl, mark))
    })
    .await?;
    Ok(Json(serde_json::json!({ "pnl": pnl, "mark": mark })))
}

async fn account(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AccountDto>, ApiErr> {
    let who = require_principal(&st, &headers).await?;
    let user = who.user;
    let min_stop_bps = st.stops.for_tenant(who.tenant.as_deref()).await;
    let m = marks(&st).await;
    let frozen = trading_frozen(&st, user).await;
    let ident = identity_for_bus(&st, user).await;
    let kind = ident.as_ref().map(|i| i.kind).unwrap_or_default();
    let account_id = ident
        .as_ref()
        .map(|i| bus::account_uuid(&i.tenant, &i.sub, i.kind));
    let b = st.broker.lock().await;

    if !b.has_account(user) {
        return Ok(Json(AccountDto {
            opened: false,
            model: None,
            balance: None,
            equity: None,
            used_margin: None,
            free_margin: None,
            margin_level_pct: None,
            stop_out_level_pct: b.stop_out_pct(),
            margin_call_level_pct: b.margin_call_pct(),
            negative_balance_protection: b.negative_balance_protection(),
            open_pnl: None,
            leverage: b.account_leverage(user),
            leverage_classes: classes_dto(&b.account_class_leverage(user)),
            markup_bps: 0,
            commission_bps: 0,
            trading_frozen: false,
            kind: kind.tag(),
            account_id,
            min_stop_bps,
            sltp_edit: st.sltp_edit_enabled,
        }));
    }

    let balance = b.balance(user).expect("счёт есть: проверено выше");
    let open_pnl = b.open_pnl(user, &m);
    let used = b.used_margin(user);
    let equity = balance + open_pnl;
    let free = b.free_margin(user, &m).expect("счёт есть: проверено выше");
    let model = match b.model(user) {
        broker::PositionModel::Netting => "netting",
        broker::PositionModel::Hedging => "hedging",
    };
    Ok(Json(AccountDto {
        opened: true,
        model: Some(model),
        balance: Some(balance),
        equity: Some(equity),
        used_margin: Some(used),
        free_margin: Some(free),
        margin_level_pct: b.margin_level_pct(user, &m),
        stop_out_level_pct: b.stop_out_pct(),
        margin_call_level_pct: b.margin_call_pct(),
        negative_balance_protection: b.negative_balance_protection(),
        open_pnl: Some(open_pnl),
        leverage: b.account_leverage(user),
        leverage_classes: classes_dto(&b.account_class_leverage(user)),
        markup_bps: b.account_fees(user).markup_bps,
        commission_bps: b.account_fees(user).commission_bps,
        trading_frozen: frozen,
        kind: b.snapshot(user).profile.kind.tag(),
        account_id,
        min_stop_bps,
        sltp_edit: st.sltp_edit_enabled,
    }))
}

#[derive(Serialize)]
struct JournalEntryDto {
    seq: u64,
    at_ms: i64,
    source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    incident: Option<String>,
    event: String,
}

#[derive(Serialize)]
struct JournalDto {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    replayed_balance: broker::Cents,
    balance: broker::Cents,
    #[serde(skip_serializing_if = "Option::is_none")]
    excluded_from_reporting_through: Option<u64>,
    entries: Vec<JournalEntryDto>,
}

async fn account_journal(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<JournalDto>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    let (snap, start_balance) = {
        let b = st.broker.lock().await;
        (b.snapshot(user), b.start_balance())
    };
    let js = st.journals.lock().await;
    let empty = Journal::new(start_balance);
    let j = js.get(&user).unwrap_or(&empty);
    let status = journal::audit(j, &snap);
    let entries = j
        .events()
        .iter()
        .map(|e| {
            let op = e.origin().operator();
            JournalEntryDto {
                seq: e.seq(),
                at_ms: e.at_ms(),
                source: e.origin().tag(),
                reason: op.map(|o| o.reason().to_string()),
                incident: op.map(|o| o.incident().to_string()),
                event: e.kind().encode(),
            }
        })
        .collect();
    Ok(Json(JournalDto {
        status: status.tag(),
        detail: if status.ok() {
            None
        } else {
            Some(status.to_string())
        },
        replayed_balance: j.replay().balance,
        balance: snap.balance,
        excluded_from_reporting_through: j.state().excluded_from_reporting_through,
        entries,
    }))
}

async fn demo_reset(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AccountDto>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    with_account_gated(&st, user, Origin::Client, Gate::NotFrozen, |b| {
        b.reset_demo(user).map(|_| ()).map_err(|e| match e {
            broker::BrokerError::NotADemoAccount => {
                err(StatusCode::FORBIDDEN, "not_a_demo_account")
            }
            broker::BrokerError::AccountNotFlat => err(StatusCode::CONFLICT, "account_not_flat"),
            broker::BrokerError::UnknownAccount => err(StatusCode::NOT_FOUND, "unknown_account"),
            _ => err(StatusCode::BAD_REQUEST, "reset_failed"),
        })
    })
    .await?;
    account(State(st), headers).await
}

#[derive(Deserialize)]
struct SessionReq {
    token: String,
    site: String,
}
#[derive(Serialize)]
struct SessionResp {
    token: String,
    expires_in: u64,
}

async fn session_create(
    State(st): State<AppState>,
    Json(req): Json<SessionReq>,
) -> Result<Json<SessionResp>, ApiErr> {
    let claims = st
        .sso
        .validate(&req.token, &req.site)
        .await
        .map_err(sso_err)?;
    let kind = match claims.account_kind.as_deref() {
        None => AccountKind::Demo,
        Some(k) => AccountKind::parse(k)
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, "unknown_account_kind"))?,
    };
    let user = st
        .store
        .resolve_account(&claims.tenant, &claims.sub, kind)
        .await
        .map_err(|_| {
            err(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity store unavailable",
            )
        })?;
    sync_leverage_claim(
        &st,
        user,
        claims.leverage,
        claims.leverage_base,
        claims.leverage_classes.as_ref(),
    )
    .await?;
    sync_fees_claim(&st, user, claims.markup_bps, claims.commission_bps).await?;
    let expires_in = st
        .sso
        .mint(claims.jti.clone(), user, claims.tenant.clone())
        .await;
    Ok(Json(SessionResp {
        token: claims.jti,
        expires_in,
    }))
}

async fn sync_leverage_claim(
    st: &AppState,
    user: UserId,
    claim: Option<i64>,
    base: Option<i64>,
    classes: Option<&std::collections::BTreeMap<String, i64>>,
) -> Result<(), ApiErr> {
    let Some(lev) = claim else {
        return Ok(());
    };
    let lev = i128::from(lev);
    if !(1..=broker::MAX_LEVERAGE).contains(&lev) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_leverage"));
    }
    let table = if st.leverage_classes_enabled {
        money::leverage_table(base, classes).map_err(|code| err(StatusCode::BAD_REQUEST, code))?
    } else {
        None
    };
    if st.quarantined.lock().await.contains(&user) {
        return Ok(());
    }
    let op = journal::Operator::new(
        "cabinet-sso",
        "плечо счёта из токена входа кабинета (портфель/override клиента)",
        "sso-login",
    )
    .expect("поля заданы константами");
    if let Err(e) = money::apply_leverage(st, user, lev, table, op).await {
        eprintln!(
            "[sso] плечо счёта {} из токена не применено ({}): вход отклонён",
            user.0, e.1 .0.error
        );
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "leverage_sync_failed"));
    }
    Ok(())
}

async fn sync_fees_claim(
    st: &AppState,
    user: UserId,
    markup: Option<i64>,
    commission: Option<i64>,
) -> Result<(), ApiErr> {
    if markup.is_none() && commission.is_none() {
        return Ok(());
    }
    let ok = |v: Option<i64>| v.is_none_or(|v| (0..=broker::MAX_FEE_BPS).contains(&v));
    if !ok(markup) || !ok(commission) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_fees"));
    }
    if st.quarantined.lock().await.contains(&user) {
        return Ok(());
    }
    let current = st.broker.lock().await.account_fees(user);
    let fees = broker::Fees {
        markup_bps: markup.unwrap_or(current.markup_bps),
        commission_bps: commission.unwrap_or(current.commission_bps),
    };
    let op = journal::Operator::new(
        "cabinet-sso",
        "тариф счёта из токена входа кабинета (портфель/override клиента)",
        "sso-login",
    )
    .expect("поля заданы константами");
    if let Err(e) = money::apply_fees(st, user, fees, op).await {
        eprintln!(
            "[sso] тариф счёта {} из токена не применён ({}): вход отклонён",
            user.0, e.1 .0.error
        );
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "fees_sync_failed"));
    }
    Ok(())
}

async fn session_logout(State(st): State<AppState>, headers: HeaderMap) -> StatusCode {
    if let Some(t) = bearer(&headers) {
        st.sso.logout(t).await;
    }
    StatusCode::NO_CONTENT
}

fn sso_err(e: sso::SsoError) -> ApiErr {
    let code = match e {
        sso::SsoError::TenantMismatch => StatusCode::FORBIDDEN,
        sso::SsoError::NoKeys => StatusCode::SERVICE_UNAVAILABLE,
        sso::SsoError::Disabled => StatusCode::NOT_IMPLEMENTED,
        sso::SsoError::BadToken | sso::SsoError::Replay => StatusCode::UNAUTHORIZED,
    };
    err(code, e.as_str())
}

pub fn spawn_monitor(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            tick.tick().await;
            monitor_tick(&st).await;
        }
    });
}

pub async fn monitor_tick(st: &AppState) {
    let m = marks(st).await;
    if m.is_empty() {
        return;
    }
    send_margin_calls(st, &m).await;

    let across_gap = st.note_feed_gap();
    let now = now_ms();
    let (exec, gap) = {
        let mut markets = st.markets.lock().unwrap_or_else(|e| e.into_inner());
        let mut exec = m.clone();
        let mut gap = std::collections::HashSet::new();
        for (id, state) in markets.iter_mut() {
            if !state.tradable(now) {
                exec.remove(id);
                state.gap = true;
            } else if state.gap {
                gap.insert(*id);
            }
        }
        (exec, gap)
    };
    let tenants: HashMap<UserId, String> = st
        .identities
        .lock()
        .await
        .iter()
        .filter_map(|(u, i)| i.as_ref().map(|i| (*u, i.tenant.clone())))
        .collect();
    let site_close_only = st.platform.site_close_only_snapshot();
    let platform_close_only: std::collections::HashSet<u32> = {
        let cat = st.catalog.read().unwrap_or_else(|e| e.into_inner());
        cat.by_id
            .keys()
            .copied()
            .filter(|id| cat.platform_mode(*id) == instruments::Mode::CloseOnly)
            .collect()
    };
    let close_only = |u: UserId, i: u32| {
        platform_close_only.contains(&i)
            || tenants
                .get(&u)
                .and_then(|t| site_close_only.get(t))
                .is_some_and(|s| s.contains(&i))
    };
    let changed = {
        let js = st.journals.lock().await;
        let frozen: std::collections::HashSet<UserId> = js
            .iter()
            .filter(|(_, j)| j.state().trading_frozen)
            .map(|(u, _)| *u)
            .collect();
        let view = broker::MarketView {
            marks: &m,
            exec: &exec,
            gap: &gap,
            close_only: &close_only,
            reject_close_only: st.platform.reject_close_only,
        };
        st.broker
            .lock()
            .await
            .check_market(&view, across_gap, &frozen)
    };
    if !gap.is_empty() {
        let mut markets = st.markets.lock().unwrap_or_else(|e| e.into_inner());
        for id in &gap {
            if let Some(state) = markets.get_mut(id) {
                state.gap = false;
            }
        }
    }
    for user in changed {
        let lock = account_lock(st, user).await;
        let _guard = lock.lock().await;
        let (snap, kinds, start_balance) = {
            let mut b = st.broker.lock().await;
            (b.snapshot(user), b.take_events(user), b.start_balance())
        };
        let violations = broker::check_snapshot(&snap);
        if !violations.is_empty() {
            st.quarantined.lock().await.insert(user);
            for v in &violations {
                eprintln!(
                    "[monitor] ИНВАРИАНТ НАРУШЕН триггером, счёт {} в карантине: {v}",
                    user.0
                );
            }
            continue;
        }
        let (sealed, before_bus) = {
            let mut js = st.journals.lock().await;
            let j = js
                .entry(user)
                .or_insert_with(|| Journal::new(start_balance));
            let before_bus = bus::Before {
                closed: j.state().closed.len(),
                balance: j.state().balance,
            };
            let sealed = j.seal(now_ms(), &Origin::System, &kinds);
            let diverged = journal::reconcile(&j.preview(&sealed), &snap);
            if !diverged.is_empty() {
                drop(js);
                st.quarantined.lock().await.insert(user);
                for d in &diverged {
                    eprintln!(
                        "[monitor] ЖУРНАЛ НЕ СХОДИТСЯ после триггера, счёт {} в карантине: {d}",
                        user.0
                    );
                }
                continue;
            }
            (sealed, before_bus)
        };
        let outgoing = bus_envelopes(st, user, before_bus, &snap, &kinds, &sealed).await;
        if let Err(e) = st.store.save_account(user, &snap, &sealed, &outgoing).await {
            eprintln!("[monitor] не удалось сохранить счёт {}: {e}", user.0);
            continue;
        }
        commit_journal(st, user, sealed).await;
    }
}

async fn send_margin_calls(st: &AppState, marks: &HashMap<u32, i64>) {
    let called = { st.broker.lock().await.margin_calls(marks) };
    for (user, level) in called {
        let lock = account_lock(st, user).await;
        let _guard = lock.lock().await;

        let (snap, equity, used, stop_out) = {
            let b = st.broker.lock().await;
            (
                b.snapshot(user),
                b.equity(user, marks),
                b.used_margin(user),
                b.stop_out_pct(),
            )
        };
        let Some(id) = identity_for_bus(st, user)
            .await
            .filter(|id| id.kind == AccountKind::Demo || st.bus_real_events)
        else {
            eprintln!(
                "[margin] счёт {} на уровне {level}% — предупредить некого: счёт не привязан к сайту",
                user.0
            );
            continue;
        };
        let seq = {
            let js = st.journals.lock().await;
            js.get(&user)
                .and_then(|j| j.events().last())
                .map(|e| e.seq())
                .unwrap_or(0)
        };
        let account_id = bus::account_uuid(&id.tenant, &id.sub, id.kind);
        let who = bus::Addressee {
            tenant: &id.tenant,
            sub: &id.sub,
            user: user.0,
            seq,
            at_ms: now_ms(),
            kind: id.kind,
            account_id: &account_id,
        };
        let equity = equity.expect("маржин-колл бывает только у существующего счёта");
        let env = bus::margin_called(&who, level, stop_out, equity, used);
        eprintln!(
            "[margin] счёт {} на уровне {level}% — предупреждение отправлено (порог закрытия {:?})",
            user.0, stop_out
        );
        if let Err(e) = st
            .store
            .save_account(user, &snap, &[], &[env.to_record()])
            .await
        {
            eprintln!(
                "[margin] ПРЕДУПРЕЖДЕНИЕ НЕ ЗАПИСАНО для счёта {}: {e} — клиент его не получит",
                user.0
            );
        }
    }
}

async fn ws_stream(State(st): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, st))
}
async fn handle_ws(mut socket: WebSocket, st: AppState) {
    let mut rx = st.events_tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(msg) => {
                if socket.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod allow_list_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const SITE: &str = "apex-ru";

    async fn tenant_app(source: TenantSource) -> (Router, String) {
        let mut st = build_state();
        st.sso = Arc::new(sso::Sso::new("http://jwks.invalid/jwks".into()));
        st.sso_enabled = true;
        st.tenant_instruments = Arc::new(source);
        let token = "session-token".to_string();
        st.sso
            .mint(token.clone(), DEFAULT_USER, SITE.to_string())
            .await;
        (router(st), token)
    }

    fn allow(site: &str, symbols: &[&str]) -> TenantSource {
        let index = symbol_index(&feed::instruments());
        let list = TenantAllowList::resolve(
            &index,
            &symbols.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        let mut m = HashMap::new();
        m.insert(site.to_string(), list);
        TenantSource::Static(m)
    }

    async fn post(
        app: &Router,
        uri: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let req = b
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        read(app.clone().oneshot(req).await.unwrap()).await
    }

    async fn get(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder().method("GET").uri(uri);
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        read(
            app.clone()
                .oneshot(b.body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await
    }

    async fn read(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, v)
    }

    fn deal(instrument: u32) -> serde_json::Value {
        serde_json::json!({"instrument": instrument, "side": "buy", "qty": 100})
    }
    fn pending(instrument: u32) -> serde_json::Value {
        serde_json::json!({"instrument": instrument, "side": "buy", "qty": 100, "price": 1000})
    }

    fn index() -> HashMap<String, u32> {
        symbol_index(&feed::instruments())
    }

    #[test]
    fn parses_source() {
        let m =
            parse_tenant_instruments(Some("apex-ru:BTCUSD,ETHUSD,BNBUSD;other:XRPUSD"), &index())
                .expect("источник задан");
        assert_eq!(
            m.get("apex-ru"),
            Some(&TenantAllowList::Resolved(vec![1, 2, 3]))
        );
        assert_eq!(m.get("other"), Some(&TenantAllowList::Resolved(vec![5])));
    }

    #[test]
    fn empty_list_is_a_value_not_absence() {
        let m = parse_tenant_instruments(Some("apex-ru:"), &index()).expect("источник задан");
        assert_eq!(m.get("apex-ru"), Some(&TenantAllowList::Resolved(vec![])));
    }

    #[test]
    fn absent_source_is_none() {
        assert!(parse_tenant_instruments(None, &index()).is_none());
        assert!(parse_tenant_instruments(Some("   "), &index()).is_none());
    }

    #[test]
    fn symbol_maps_to_internal_id() {
        let idx = index();
        for s in ["BTCUSD", "BTCUSDT", "BTC-USDT", "btcusd", "btc-usdt"] {
            assert_eq!(
                idx.get(&normalize_symbol(s)),
                Some(&1),
                "символ {s} обязан указывать на инструмент 1"
            );
        }
        assert_eq!(idx.get(&normalize_symbol("MANAUSD")), Some(&30));
    }

    #[test]
    fn symbol_index_has_no_collisions() {
        let instruments = feed::instruments();
        let idx = symbol_index(&instruments);
        assert_eq!(
            idx.len(),
            instruments.len() * 2,
            "каждый символ обязан указывать ровно на один инструмент"
        );
        for f in &instruments {
            assert_eq!(idx.get(&normalize_symbol(&f.binance)), Some(&f.id));
            assert_eq!(idx.get(&normalize_symbol(&f.symbol)), Some(&f.id));
        }
    }

    #[test]
    fn unknown_symbol_rejects_the_whole_list() {
        let list = TenantAllowList::resolve(
            &index(),
            &["BTCUSD".into(), "XAUUSD".into(), "ETHUSD".into()],
        );
        match list {
            TenantAllowList::Unknown(unknown) => assert_eq!(unknown, vec!["XAUUSD".to_string()]),
            TenantAllowList::Resolved(ids) => {
                panic!(
                    "неизвестный символ не должен молча исчезать, а список — сужаться до {ids:?}"
                )
            }
        }
    }

    #[tokio::test]
    async fn unknown_symbol_closes_trading_with_its_own_reason() {
        let (app, tok) = tenant_app(allow(SITE, &["BTCUSD", "XAUUSD"])).await;
        let (status, b) = post(&app, "/deals", Some(&tok), deal(1)).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "список с неизвестным символом не принимается целиком"
        );
        assert_eq!(
            b["error"], "tenant_instruments_unreadable",
            "причина обязана отличаться от «источника нет»"
        );
    }

    #[tokio::test]
    async fn open_deal_rejects_instrument_outside_allow_list() {
        let (app, tok) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let (status, body) = post(&app, "/deals", Some(&tok), deal(2)).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "инструмент вне allow-list не должен торговаться"
        );
        assert_eq!(body["error"], "instrument_not_allowed");
    }

    #[tokio::test]
    async fn pending_rejects_instrument_outside_allow_list() {
        let (app, tok) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let (status, body) = post(&app, "/pending", Some(&tok), pending(2)).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "отложенный ордер обязан проверяться так же, как рыночный"
        );
        assert_eq!(body["error"], "instrument_not_allowed");
    }

    #[tokio::test]
    async fn allowed_instrument_passes() {
        let (app, tok) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let (status, _) = post(&app, "/pending", Some(&tok), pending(1)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "разрешённый инструмент обязан торговаться"
        );
    }

    #[tokio::test]
    async fn empty_allow_list_denies_everything() {
        let (app, tok) = tenant_app(allow(SITE, &[])).await;
        let (status, _) = post(&app, "/deals", Some(&tok), deal(1)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn missing_source_closes_trading() {
        let (app, tok) = tenant_app(TenantSource::None).await;
        for (uri, body) in [("/deals", deal(1)), ("/pending", pending(1))] {
            let (status, b) = post(&app, uri, Some(&tok), body).await;
            assert_eq!(
                status,
                StatusCode::SERVICE_UNAVAILABLE,
                "{uri}: без конфига торговли быть не должно"
            );
            assert_eq!(
                b["error"], "tenant_instruments_unavailable",
                "{uri}: причина обязана отличаться от «не разрешено»"
            );
        }
    }

    #[tokio::test]
    async fn unknown_site_closes_trading() {
        let (app, tok) = tenant_app(allow("some-other-site", &["BTCUSD", "ETHUSD"])).await;
        let (status, b) = post(&app, "/deals", Some(&tok), deal(1)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(b["error"], "tenant_instruments_unavailable");
    }

    #[tokio::test]
    async fn closing_is_never_blocked_by_allow_list() {
        let (app, tok) = tenant_app(allow(SITE, &[])).await;
        let (status, _) = post(&app, "/deals/1/close", Some(&tok), serde_json::Value::Null).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "закрытие не должно упираться в allow-list"
        );
    }

    #[tokio::test]
    async fn cancelling_is_never_blocked_by_allow_list() {
        let (app, tok) = tenant_app(allow(SITE, &[])).await;
        let req = Request::builder()
            .method("DELETE")
            .uri("/pending/1")
            .header("authorization", format!("Bearer {tok}"))
            .body(Body::empty())
            .unwrap();
        let (status, _) = read(app.oneshot(req).await.unwrap()).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "отмена ордера не должна упираться в allow-list"
        );
    }

    #[tokio::test]
    async fn sl_tp_travel_with_the_position_not_as_orders() {
        let (app, tok) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let body = serde_json::json!({"instrument": 1, "side": "buy", "qty": 100, "price": 1000, "sl": 900, "tp": 1200});
        let (status, v) = post(&app, "/pending", Some(&tok), body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["sl"], 900);
        assert_eq!(v["tp"], 1200);
    }

    #[tokio::test]
    async fn reading_account_is_not_blocked() {
        let (app, tok) = tenant_app(allow(SITE, &[])).await;
        let (status, _) = get(&app, "/account", Some(&tok)).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn instruments_listing_filtered_by_site() {
        let (app, _) = tenant_app(allow(SITE, &["ETHUSD", "BNBUSD"])).await;
        let (status, body) = get(&app, "/instruments?site=apex-ru", None).await;
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<u64> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_u64().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![2, 3],
            "витрина обязана показывать только разрешённое"
        );
    }

    #[tokio::test]
    async fn instruments_listing_fails_loudly_without_config() {
        let (app, _) = tenant_app(TenantSource::None).await;
        let (status, b) = get(&app, "/instruments?site=apex-ru", None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(b["error"], "tenant_instruments_unavailable");
    }

    #[tokio::test]
    async fn empty_allow_list_is_said_out_loud_not_rendered_as_empty() {
        let (app, _) = tenant_app(allow(SITE, &[])).await;
        let (status, b) = get(&app, "/instruments?site=apex-ru", None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "пустой список инструментов не отдаётся никогда"
        );
        assert_eq!(b["error"], "no_instruments_allowed");
    }

    #[tokio::test]
    async fn change_is_unknown_not_zero_when_there_is_no_ticker() {
        let (app, _) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let (status, body) = get(&app, "/instruments?site=apex-ru", None).await;
        assert_eq!(status, StatusCode::OK);

        let row = &body.as_array().expect("список")[0];
        assert!(row["last"].is_null(), "предпосылка теста: тикера нет");
        assert!(
            row["change"].is_null(),
            "изменение без котировки обязано быть null, а не {}",
            row["change"]
        );
    }

    #[tokio::test]
    async fn instruments_listing_unfiltered_without_site() {
        let (app, _) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let (status, body) = get(&app, "/instruments", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_array().unwrap().len(), feed::instruments().len());
    }

    #[tokio::test]
    async fn standalone_is_not_restricted() {
        let app = router(build_state());
        let (status, _) = post(&app, "/pending", None, pending(30)).await;
        assert_eq!(status, StatusCode::OK);
    }
}

#[cfg(test)]
mod invariant_wiring_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn corrupt_snapshot() -> broker::AccountSnapshot {
        broker::AccountSnapshot {
            profile: Default::default(),
            balance: 1_000_000,
            next_id: 1,
            positions: vec![broker::Position {
                id: 1,
                instrument: 1,
                side: PosSide::Long,
                qty: 0,
                entry: 5000,
                pd: 2,
                qd: 3,
                margin: 5_000,
                sl: None,
                tp: None,
                leverage: 1,
                fees: Default::default(),
            }],
            pendings: vec![],
            closed: vec![],
            model: broker::PositionModel::Hedging,
            leverage: None,
            class_leverage: Default::default(),
            fees: Default::default(),
        }
    }

    struct CorruptStore;
    #[async_trait::async_trait]
    impl persistence::Persistence for CorruptStore {
        async fn init(&self) -> Result<(), persistence::StoreError> {
            Ok(())
        }
        async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
            Ok(persistence::LoadedState {
                accounts: vec![(DEFAULT_USER, corrupt_snapshot())],
                journals: vec![],
            })
        }
        async fn save_account(
            &self,
            _u: UserId,
            _s: &broker::AccountSnapshot,
            _e: &[journal::AccountEvent],
            _out: &[persistence::OutboxRecord],
        ) -> Result<(), persistence::StoreError> {
            Ok(())
        }
    }

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let b = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, v)
    }

    fn pending_body() -> serde_json::Value {
        serde_json::json!({"instrument": 1, "side": "buy", "qty": 100, "price": 1000})
    }

    #[tokio::test]
    async fn mutation_over_broken_state_is_rejected_before_write() {
        let st = build_state();
        st.broker
            .lock()
            .await
            .restore(DEFAULT_USER, corrupt_snapshot());
        let app = router(st.clone());

        let (status, body) = send(&app, "POST", "/pending", Some(pending_body())).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "операция, оставляющая счёт неверным, обязана быть отклонена"
        );
        assert_eq!(
            body["error"], "invariant_violated",
            "отказ обязан отличаться от делового"
        );

        let (_, list) = send(&app, "GET", "/pending", None).await;
        assert_eq!(
            list.as_array().map(|a| a.len()),
            Some(0),
            "изменение не должно фиксироваться"
        );
    }

    #[tokio::test]
    async fn invariant_rejection_differs_from_business_rejection() {
        let st = build_state();
        st.broker
            .lock()
            .await
            .restore(DEFAULT_USER, corrupt_snapshot());
        let app = router(st);
        let (status, body) = send(&app, "POST", "/pending", Some(pending_body())).await;
        assert_ne!(status, StatusCode::PAYMENT_REQUIRED);
        assert_ne!(body["error"], "insufficient_margin");
    }

    #[tokio::test]
    async fn broken_state_quarantines_account_but_service_starts() {
        let st = build_state_with(Arc::new(CorruptStore));
        restore_state(&st)
            .await
            .expect("сервис обязан стартовать, а не падать");
        assert!(
            st.quarantined.lock().await.contains(&DEFAULT_USER),
            "счёт обязан уйти в карантин"
        );
    }

    #[tokio::test]
    async fn quarantined_account_reads_but_does_not_mutate() {
        let st = build_state_with(Arc::new(CorruptStore));
        restore_state(&st).await.unwrap();
        let app = router(st);

        let (read_status, _) = send(&app, "GET", "/account", None).await;
        assert_eq!(
            read_status,
            StatusCode::OK,
            "чтение в карантине обязано работать"
        );

        let (write_status, body) = send(&app, "POST", "/pending", Some(pending_body())).await;
        assert_eq!(
            write_status,
            StatusCode::LOCKED,
            "мутации в карантине обязаны отклоняться"
        );
        assert_eq!(body["error"], "account_quarantined");
    }

    #[tokio::test]
    async fn broken_state_is_not_silently_repaired() {
        let st = build_state_with(Arc::new(CorruptStore));
        restore_state(&st).await.unwrap();
        let snap = st.broker.lock().await.snapshot(DEFAULT_USER);
        assert_eq!(snap.positions.len(), 1, "позиция не должна исчезнуть");
        assert_eq!(
            snap.positions[0].qty, 0,
            "нулевой объём обязан сохраниться как улика"
        );
        assert!(
            !broker::check_snapshot(&snap).is_empty(),
            "состояние обязано остаться помеченным как неверное"
        );
    }

    #[tokio::test]
    async fn healthy_account_is_not_quarantined() {
        let st = build_state();
        restore_state(&st).await.unwrap();
        assert!(st.quarantined.lock().await.is_empty());
        let app = router(st);
        let (status, _) = send(&app, "POST", "/pending", Some(pending_body())).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "нормальная торговля не должна страдать от проверок"
        );
    }
}

#[cfg(test)]
mod journal_wiring_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let b = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, v)
    }

    async fn with_price(st: &AppState, instrument: u32, px: i64) {
        st.tickers.lock().await.insert(
            instrument,
            feed::Ticker {
                last: px,
                change: 0.0,
                high: px,
                bid: px,
                ask: px,
                ..Default::default()
            },
        );
    }

    fn deal_body() -> serde_json::Value {
        serde_json::json!({"instrument": 1, "side": "buy", "qty": 1000})
    }

    async fn events_of(st: &AppState) -> Vec<String> {
        st.journals
            .lock()
            .await
            .get(&DEFAULT_USER)
            .map(|j| j.events().iter().map(|e| e.kind().encode()).collect())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn opening_a_deal_through_the_router_lands_in_the_journal() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());

        let (status, _) = send(&app, "POST", "/deals", Some(deal_body())).await;
        assert_eq!(status, StatusCode::OK);

        let kinds = events_of(&st).await;
        assert!(
            kinds.iter().any(|k| k.starts_with("account_opened")),
            "заведение счёта обязано быть записано: {kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| k.starts_with("position_opened")),
            "сделка обязана быть записана: {kinds:?}"
        );

        let (status, body) = send(&app, "GET", "/account/journal", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["status"], "reconciled",
            "журнал обязан сходиться со слепком: {body}"
        );
        assert_eq!(
            body["replayed_balance"], body["balance"],
            "остаток обязан выводиться из журнала"
        );
        assert_eq!(body["entries"].as_array().unwrap().len(), 2);
        assert_eq!(
            body["entries"][1]["source"], "client",
            "сделку сделал человек, и это видно"
        );
    }

    #[tokio::test]
    async fn closing_through_the_router_is_recorded_with_its_cause() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());
        let (_, deal) = send(&app, "POST", "/deals", Some(deal_body())).await;
        let id = deal["id"].as_u64().unwrap();

        let (status, _) = send(&app, "POST", &format!("/deals/{id}/close"), None).await;
        assert_eq!(status, StatusCode::OK);

        let kinds = events_of(&st).await;
        assert!(
            kinds
                .iter()
                .any(|k| k.contains("position_closed") && k.contains("cause=command")),
            "{kinds:?}"
        );
        let (_, body) = send(&app, "GET", "/account/journal", None).await;
        assert_eq!(body["status"], "reconciled");
    }

    #[tokio::test]
    async fn pending_order_and_its_cancellation_are_recorded() {
        let st = build_state();
        let app = router(st.clone());
        let body = serde_json::json!({"instrument": 1, "side": "buy", "qty": 100, "price": 1000});
        let (_, p) = send(&app, "POST", "/pending", Some(body)).await;
        let id = p["id"].as_u64().unwrap();
        send(&app, "DELETE", &format!("/pending/{id}"), None).await;

        let kinds = events_of(&st).await;
        assert!(
            kinds.iter().any(|k| k.starts_with("pending_placed")),
            "{kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| k.starts_with("pending_cancelled")),
            "{kinds:?}"
        );
    }

    #[tokio::test]
    async fn closed_deal_is_served_with_level_and_gap() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());
        let body = serde_json::json!({"instrument": 1, "side": "buy", "qty": 1000, "sl": 49_000});
        let (status, _) = send(&app, "POST", "/deals", Some(body)).await;
        assert_eq!(status, StatusCode::OK);

        with_price(&st, 1, 48_000).await;
        monitor_tick(&st).await;

        let (status, v) = send(&app, "GET", "/deals/closed", None).await;
        assert_eq!(status, StatusCode::OK);
        let d = &v.as_array().expect("список сделок")[0];

        assert_eq!(d["exit"], 48_000, "цена исполнения");
        assert_eq!(
            d["level"], 49_000,
            "и поставленный уровень — без него проскальзывание не посчитать"
        );
        assert!(
            !d["across_gap"].is_null(),
            "признак разрыва обязан присутствовать: отсутствие означает «неизвестно», а здесь известно"
        );
    }

    #[tokio::test]
    async fn stop_loss_fired_by_the_monitor_is_recorded_as_a_system_event() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());
        let body = serde_json::json!({"instrument": 1, "side": "buy", "qty": 1000, "sl": 49_000});
        let (status, _) = send(&app, "POST", "/deals", Some(body)).await;
        assert_eq!(status, StatusCode::OK);

        with_price(&st, 1, 48_000).await;
        monitor_tick(&st).await;

        let kinds = events_of(&st).await;
        assert!(
            kinds
                .iter()
                .any(|k| k.contains("position_closed") && k.contains("cause=stop_loss")),
            "срабатывание стопа обязано попасть в журнал: {kinds:?}"
        );
        let last = st
            .journals
            .lock()
            .await
            .get(&DEFAULT_USER)
            .unwrap()
            .events()
            .last()
            .unwrap()
            .clone();
        assert_eq!(
            last.origin(),
            &Origin::System,
            "у триггера нет человека-автора"
        );

        let (_, jb) = send(&app, "GET", "/account/journal", None).await;
        assert_eq!(
            jb["status"], "reconciled",
            "после срабатывания стопа остаток обязан сходиться: {jb}"
        );
        assert!(
            st.quarantined.lock().await.is_empty(),
            "нормальное срабатывание — не повод для карантина"
        );
    }

    #[tokio::test]
    async fn journal_survives_a_restart_through_the_store() {
        let store = Arc::new(persistence::MemoryStore::new());
        let before = {
            let st = build_state_with(store.clone());
            with_price(&st, 1, 50_000).await;
            let app = router(st.clone());
            send(&app, "POST", "/deals", Some(deal_body())).await;
            send(
                &app,
                "POST",
                "/pending",
                Some(
                    serde_json::json!({"instrument": 2, "side": "sell", "qty": 10, "price": 9_000}),
                ),
            )
            .await;
            let (_, acct) = send(&app, "GET", "/account", None).await;
            acct["balance"].clone()
        };

        let st2 = build_state_with(store.clone());
        restore_state(&st2).await.expect("подъём обязан пройти");
        let app2 = router(st2.clone());

        assert!(
            st2.quarantined.lock().await.is_empty(),
            "сошедшийся счёт в карантин не уходит"
        );
        let (_, jb) = send(&app2, "GET", "/account/journal", None).await;
        assert_eq!(
            jb["status"], "reconciled",
            "после рестарта журнал обязан сходиться: {jb}"
        );
        assert_eq!(jb["balance"], before, "остаток обязан пережить рестарт");
        assert_eq!(
            jb["replayed_balance"], before,
            "и обязан выводиться из журнала"
        );
        assert!(
            jb["entries"].as_array().unwrap().len() >= 3,
            "лента обязана подняться целиком: {jb}"
        );
    }

    #[tokio::test]
    async fn a_state_change_without_a_journal_entry_is_rejected() {
        let st = build_state();
        let app = router(st.clone());
        send(
            &app,
            "POST",
            "/pending",
            Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 1, "price": 10})),
        )
        .await;
        let before = st.broker.lock().await.balance(DEFAULT_USER);

        let out: Result<(), ApiErr> = with_account(&st, DEFAULT_USER, Origin::Client, |b| {
            let mut snap = b.snapshot(DEFAULT_USER);
            snap.balance += 777_000;
            b.restore(DEFAULT_USER, snap);
            Ok(())
        })
        .await;
        assert!(out.is_err(), "несписанное изменение обязано быть отклонено");
        assert_eq!(
            st.broker.lock().await.balance(DEFAULT_USER),
            before,
            "и откачено: деньги без проводки не остаются на счёте"
        );
    }

    #[tokio::test]
    async fn an_account_without_a_journal_is_quarantined_as_no_entries() {
        struct SnapshotOnlyStore;
        #[async_trait::async_trait]
        impl persistence::Persistence for SnapshotOnlyStore {
            async fn init(&self) -> Result<(), persistence::StoreError> {
                Ok(())
            }
            async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
                let snap = broker::AccountSnapshot {
                    profile: Default::default(),
                    balance: 42_000_000,
                    next_id: 0,
                    ..Default::default()
                };
                Ok(persistence::LoadedState {
                    accounts: vec![(DEFAULT_USER, snap)],
                    journals: vec![],
                })
            }
            async fn save_account(
                &self,
                _u: UserId,
                _s: &broker::AccountSnapshot,
                _e: &[journal::AccountEvent],
                _out: &[persistence::OutboxRecord],
            ) -> Result<(), persistence::StoreError> {
                Ok(())
            }
        }
        let st = build_state_with(Arc::new(SnapshotOnlyStore));
        restore_state(&st)
            .await
            .expect("сервис обязан стартовать: стоп сервиса не применяется нигде");
        assert!(
            st.quarantined.lock().await.contains(&DEFAULT_USER),
            "счёт без истории обязан уйти в карантин"
        );

        let app = router(st);
        let (status, body) = send(&app, "GET", "/account/journal", None).await;
        assert_eq!(status, StatusCode::OK, "читать можно и в карантине");
        assert_eq!(
            body["status"], "no_entries",
            "диагноз обязан называть беду своим именем: {body}"
        );
    }

    #[tokio::test]
    async fn unreadable_storage_is_not_the_same_as_an_empty_journal() {
        struct UnreadableStore;
        #[async_trait::async_trait]
        impl persistence::Persistence for UnreadableStore {
            async fn init(&self) -> Result<(), persistence::StoreError> {
                Ok(())
            }
            async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
                Err(persistence::StoreError("журнал недоступен".into()))
            }
            async fn save_account(
                &self,
                _u: UserId,
                _s: &broker::AccountSnapshot,
                _e: &[journal::AccountEvent],
                _out: &[persistence::OutboxRecord],
            ) -> Result<(), persistence::StoreError> {
                Ok(())
            }
        }
        let st = build_state_with(Arc::new(UnreadableStore));
        let out = restore_state(&st).await;
        assert!(out.is_err(), "на недоступном журнале сервис не стартует");
        assert!(
            st.quarantined.lock().await.is_empty(),
            "карантин тут ни при чём: мы вообще ничего не прочитали"
        );
    }

    #[tokio::test]
    async fn a_tampered_journal_quarantines_the_account_and_is_not_repaired() {
        let store = Arc::new(persistence::MemoryStore::new());
        {
            let st = build_state_with(store.clone());
            let app = router(st.clone());
            send(
                &app,
                "POST",
                "/pending",
                Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 5, "price": 100})),
            )
            .await;
        }
        struct Tampered(Arc<persistence::MemoryStore>);
        #[async_trait::async_trait]
        impl persistence::Persistence for Tampered {
            async fn init(&self) -> Result<(), persistence::StoreError> {
                Ok(())
            }
            async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
                let mut loaded = self.0.load_all().await?;
                for (_, lane) in loaded.journals.iter_mut() {
                    let v = &lane[0];
                    lane[0] = journal::AccountEvent::from_storage(
                        v.seq(),
                        v.at_ms(),
                        v.origin().clone(),
                        &journal::EventKind::AccountOpened {
                            balance: 99_000_000,
                            profile: Default::default(),
                        }
                        .encode(),
                        v.prev(),
                        v.hash(),
                    )
                    .expect("подделанная строка обязана разбираться");
                }
                Ok(loaded)
            }
            async fn save_account(
                &self,
                u: UserId,
                s: &broker::AccountSnapshot,
                e: &[journal::AccountEvent],
                out: &[persistence::OutboxRecord],
            ) -> Result<(), persistence::StoreError> {
                self.0.save_account(u, s, e, out).await
            }
        }
        let st2 = build_state_with(Arc::new(Tampered(store.clone())));
        restore_state(&st2).await.expect("сервис обязан стартовать");
        assert!(
            st2.quarantined.lock().await.contains(&DEFAULT_USER),
            "подделанная лента — карантин"
        );

        let app2 = router(st2.clone());
        let (status, _) = send(
            &app2,
            "POST",
            "/pending",
            Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 1, "price": 1})),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::LOCKED,
            "мутации по счёту с испорченной историей не проходят"
        );
        let snap = st2.broker.lock().await.snapshot(DEFAULT_USER);
        assert_eq!(
            snap.pendings.len(),
            1,
            "состояние не чинится и не затирается: улика остаётся"
        );
    }

    #[tokio::test]
    async fn a_failed_write_leaves_no_entry_in_the_journal() {
        struct FailingStore;
        #[async_trait::async_trait]
        impl persistence::Persistence for FailingStore {
            async fn init(&self) -> Result<(), persistence::StoreError> {
                Ok(())
            }
            async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
                Ok(persistence::LoadedState::default())
            }
            async fn save_account(
                &self,
                _u: UserId,
                _s: &broker::AccountSnapshot,
                _e: &[journal::AccountEvent],
                _out: &[persistence::OutboxRecord],
            ) -> Result<(), persistence::StoreError> {
                Err(persistence::StoreError("диск горит".into()))
            }
        }
        let st = build_state_with(Arc::new(FailingStore));
        let app = router(st.clone());
        let (status, _) = send(
            &app,
            "POST",
            "/pending",
            Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 1, "price": 10})),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            events_of(&st).await.is_empty(),
            "незаписанного в хранилище в журнале быть не должно"
        );
    }

    #[tokio::test]
    async fn a_real_account_cannot_open_a_position() {
        let st = build_state();
        {
            let mut b = st.broker.lock().await;
            b.open_account_with(
                DEFAULT_USER,
                broker::AccountProfile {
                    owner: DEFAULT_USER,
                    kind: broker::AccountKind::Real,
                    ..Default::default()
                },
                0,
            );
        }
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        let (status, body) = send(&app, "POST", "/deals", Some(deal_body())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["error"], "identity_not_verified", "{body}");
    }

    #[tokio::test]
    async fn a_demo_account_still_trades() {
        let st = build_state();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        let (status, _) = send(&app, "POST", "/deals", Some(deal_body())).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_new_account_is_opened_with_its_profile() {
        let st = build_state();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        assert_eq!(
            send(&app, "POST", "/deals", Some(deal_body())).await.0,
            StatusCode::OK
        );

        let kinds = events_of(&st).await;
        assert!(
            kinds[0].starts_with("account_opened")
                && kinds[0].contains("kind=demo")
                && kinds[0].contains("origin=registration")
                && kinds[0].contains("category_at_open=retail")
                && kinds[0].contains("currency=USD"),
            "профиль обязан быть в записи о заведении: {kinds:?}"
        );
    }

    #[tokio::test]
    async fn a_reset_with_an_open_position_is_refused_by_its_own_code() {
        let st = build_state();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        assert_eq!(
            send(&app, "POST", "/deals", Some(deal_body())).await.0,
            StatusCode::OK
        );

        let (status, body) = send(&app, "POST", "/account/demo-reset", None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"], "account_not_flat", "{body}");
    }

    #[tokio::test]
    async fn a_rejected_first_operation_still_records_the_account_opening() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());

        let huge = serde_json::json!({"instrument": 1, "side": "buy", "qty": 100_000_000});
        let (status, body) = send(&app, "POST", "/deals", Some(huge)).await;
        assert_ne!(status, StatusCode::OK, "операция обязана отказать: {body}");
        assert_eq!(
            body["error"], "insufficient_margin",
            "отказ обязан быть деловым, а не посторонним: {body}"
        );

        let kinds = events_of(&st).await;
        assert_eq!(
            kinds.len(),
            1,
            "в журнале обязана быть ровно одна запись — о заведении: {kinds:?}"
        );
        assert!(
            kinds[0].starts_with("account_opened"),
            "первые деньги на счёте обязаны иметь происхождение: {kinds:?}"
        );
        assert!(
            kinds.iter().all(|k| !k.starts_with("position_opened")),
            "несостоявшаяся операция в журнал попасть не должна: {kinds:?}"
        );
    }

    #[tokio::test]
    async fn a_level_already_reached_is_refused_by_its_own_name() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());

        let (status, body) = send(
            &app,
            "POST",
            "/deals",
            Some(serde_json::json!({
                "instrument": 1, "side": "buy", "qty": 1000, "tp": 820
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"], "take_profit_already_reached", "{body}");
        assert_ne!(
            body["error"], "insufficient_margin",
            "денег хватало — отказ не про них: {body}"
        );

        let (status, body) = send(
            &app,
            "POST",
            "/deals",
            Some(serde_json::json!({
                "instrument": 1, "side": "buy", "qty": 1000, "sl": 90_000
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"], "stop_loss_already_reached", "{body}");

        let (status, _) = send(
            &app,
            "POST",
            "/deals",
            Some(serde_json::json!({
                "instrument": 1, "side": "buy", "qty": 1000, "sl": 40_000, "tp": 60_000
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_rejected_close_still_records_the_account_opening() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());

        let (status, _) = send(&app, "POST", "/deals/777/close", None).await;
        assert_ne!(status, StatusCode::OK, "позиции нет — закрывать нечего");

        let kinds = events_of(&st).await;
        assert_eq!(kinds.len(), 1, "{kinds:?}");
        assert!(kinds[0].starts_with("account_opened"), "{kinds:?}");
    }

    #[tokio::test]
    async fn a_rejected_cancel_still_records_the_account_opening() {
        let st = build_state();
        let app = router(st.clone());

        let (status, _) = send(&app, "DELETE", "/pending/777", None).await;
        assert_ne!(status, StatusCode::OK, "ордера нет — снимать нечего");

        let kinds = events_of(&st).await;
        assert_eq!(kinds.len(), 1, "{kinds:?}");
        assert!(kinds[0].starts_with("account_opened"), "{kinds:?}");
    }

    #[tokio::test]
    async fn the_opening_is_its_own_entry_and_comes_first() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());
        let (status, _) = send(&app, "POST", "/deals", Some(deal_body())).await;
        assert_eq!(status, StatusCode::OK);

        let kinds = events_of(&st).await;
        assert!(kinds[0].starts_with("account_opened"), "{kinds:?}");
        assert_eq!(
            kinds
                .iter()
                .filter(|k| k.starts_with("account_opened"))
                .count(),
            1,
            "счёт заводится один раз: {kinds:?}"
        );
        assert!(kinds[1].starts_with("position_opened"), "{kinds:?}");
    }

    #[tokio::test]
    async fn an_account_whose_opening_was_not_written_does_not_stay_in_memory() {
        struct FailOnce(std::sync::atomic::AtomicBool);
        #[async_trait::async_trait]
        impl persistence::Persistence for FailOnce {
            async fn init(&self) -> Result<(), persistence::StoreError> {
                Ok(())
            }
            async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
                Ok(persistence::LoadedState::default())
            }
            async fn save_account(
                &self,
                _u: UserId,
                _s: &broker::AccountSnapshot,
                _e: &[journal::AccountEvent],
                _o: &[persistence::OutboxRecord],
            ) -> Result<(), persistence::StoreError> {
                if self.0.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    return Err(persistence::StoreError("диск моргнул".into()));
                }
                Ok(())
            }
        }

        let st = build_state_with(Arc::new(FailOnce(std::sync::atomic::AtomicBool::new(true))));
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());

        let (status, _) = send(&app, "POST", "/deals", Some(deal_body())).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "запись не прошла");
        assert!(
            !st.broker.lock().await.has_account(DEFAULT_USER),
            "незаписанный счёт обязан исчезнуть и из памяти"
        );

        let (status, _) = send(&app, "POST", "/deals", Some(deal_body())).await;
        assert_eq!(status, StatusCode::OK);
        let kinds = events_of(&st).await;
        assert!(
            kinds
                .first()
                .is_some_and(|k| k.starts_with("account_opened")),
            "после отката происхождение обязано записаться: {kinds:?}"
        );
    }
}

#[cfg(test)]
mod bus_wiring_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TENANT: &str = "apex-ru";
    const SUB: &str = "sub-777";

    struct Tenanted {
        inner: persistence::MemoryStore,
        linked: UserId,
    }

    #[async_trait::async_trait]
    impl Persistence for Tenanted {
        async fn init(&self) -> Result<(), persistence::StoreError> {
            self.inner.init().await
        }
        async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
            self.inner.load_all().await
        }
        async fn save_account(
            &self,
            u: UserId,
            s: &broker::AccountSnapshot,
            e: &[journal::AccountEvent],
            o: &[persistence::OutboxRecord],
        ) -> Result<(), persistence::StoreError> {
            self.inner.save_account(u, s, e, o).await
        }
        async fn claim_outbox(
            &self,
            limit: i64,
            lease_ms: i64,
        ) -> Result<Vec<persistence::OutboxClaim>, persistence::StoreError> {
            self.inner.claim_outbox(limit, lease_ms).await
        }
        async fn mark_published(&self, id: i64) -> Result<(), persistence::StoreError> {
            self.inner.mark_published(id).await
        }
        async fn outbox_backlog(&self) -> Result<i64, persistence::StoreError> {
            self.inner.outbox_backlog().await
        }
        async fn identity_of(
            &self,
            u: UserId,
        ) -> Result<Option<(String, String)>, persistence::StoreError> {
            Ok((u == self.linked).then(|| (TENANT.to_string(), SUB.to_string())))
        }
    }

    async fn stub_cms(body: Option<serde_json::Value>) -> String {
        use axum::response::IntoResponse;
        let app = Router::new().route(
            "/cms/sites/:slug",
            axum::routing::get(move || {
                let b = body.clone();
                async move {
                    match b {
                        Some(v) => axum::Json(v).into_response(),
                        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    }
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        format!("http://{addr}")
    }

    fn with_cms(mut st: AppState, base: String) -> AppState {
        st.tenant_instruments = Arc::new(TenantSource::Cms {
            base,
            api_key: "k".to_string(),
            http: reqwest::Client::new(),
        });
        st
    }

    async fn get_json(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    async fn post_json(
        app: &Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    fn deal() -> serde_json::Value {
        serde_json::json!({"instrument": 1, "side": "buy", "qty": 1000})
    }

    #[tokio::test]
    async fn the_account_opens_with_the_amount_the_site_named() {
        let base = stub_cms(Some(serde_json::json!({
            "instruments": ["BTCUSD"],
            "demoStartBalanceCents": 77_700
        })))
        .await;
        let st = with_cms(build_state_with(tenanted(DEFAULT_USER)), base);
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        assert_eq!(post_json(&app, "/deals", deal()).await.0, StatusCode::OK);
        let (_, acc) = get_json(&app, "/account").await;
        assert_eq!(
            acc["balance"],
            serde_json::json!(77_700),
            "счёт открыт суммой, которую назвал сайт: {acc}"
        );
        assert_ne!(
            acc["balance"], 10_000_000,
            "настройка терминала при живой CMS не смотрится вовсе: {acc}"
        );
    }

    #[tokio::test]
    async fn zero_is_a_legal_start_balance() {
        let base = stub_cms(Some(serde_json::json!({
            "instruments": ["BTCUSD"],
            "demoStartBalanceCents": 0
        })))
        .await;
        let st = with_cms(build_state_with(tenanted(DEFAULT_USER)), base);
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        let (status, _) = post_json(&app, "/deals", deal()).await;
        assert_eq!(
            status,
            StatusCode::PAYMENT_REQUIRED,
            "денег нет — отказ по марже"
        );
        let (_, acc) = get_json(&app, "/account").await;
        assert_eq!(acc["opened"], serde_json::json!(true), "{acc}");
        assert_eq!(acc["balance"], serde_json::json!(0), "{acc}");
    }

    #[tokio::test]
    async fn an_unnamed_start_balance_refuses_instead_of_inventing_one() {
        let base = stub_cms(Some(serde_json::json!({ "instruments": ["BTCUSD"] }))).await;
        let st = with_cms(build_state_with(tenanted(DEFAULT_USER)), base);
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        let (status, body) = post_json(&app, "/deals", deal()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"], "demo_start_balance_not_set", "{body}");

        let (_, acc) = get_json(&app, "/account").await;
        assert_eq!(
            acc["opened"],
            serde_json::json!(false),
            "несостоявшееся заведение не оставляет счёта: {acc}"
        );
    }

    #[tokio::test]
    async fn an_unreachable_source_is_a_different_refusal() {
        let base = stub_cms(None).await;
        let st = with_cms(build_state_with(tenanted(DEFAULT_USER)), base);
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        let (status, body) = post_json(&app, "/deals", deal()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(body["error"], "demo_start_balance_unavailable", "{body}");
        assert_ne!(
            body["error"], "demo_start_balance_not_set",
            "«не ответил» и «не сказал» чинят разные люди: {body}"
        );
    }

    struct BlindIdentity(persistence::MemoryStore);

    #[async_trait::async_trait]
    impl Persistence for BlindIdentity {
        async fn init(&self) -> Result<(), persistence::StoreError> {
            self.0.init().await
        }
        async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
            self.0.load_all().await
        }
        async fn save_account(
            &self,
            u: UserId,
            s: &broker::AccountSnapshot,
            e: &[journal::AccountEvent],
            o: &[persistence::OutboxRecord],
        ) -> Result<(), persistence::StoreError> {
            self.0.save_account(u, s, e, o).await
        }
        async fn identity_of(
            &self,
            _u: UserId,
        ) -> Result<Option<(String, String)>, persistence::StoreError> {
            Err(persistence::StoreError("связь с базой потеряна".into()))
        }
    }

    #[test]
    fn demo_start_from_env_treats_zero_as_a_value() {
        assert_eq!(super::demo_start_from_env(None), 10_000_000, "не задано");
        assert_eq!(super::demo_start_from_env(Some("")), 10_000_000, "пусто");
        assert_eq!(super::demo_start_from_env(Some("777")), 777);
        assert_eq!(
            super::demo_start_from_env(Some("0")),
            0,
            "ноль — решение, а не отсутствие значения"
        );
        assert_eq!(
            super::demo_start_from_env(Some("-5")),
            10_000_000,
            "отрицательная сумма — испорченная настройка, не намерение"
        );
        assert_eq!(super::demo_start_from_env(Some("abc")), 10_000_000);
    }

    #[tokio::test]
    async fn opening_an_account_does_not_look_like_a_balance_change() {
        let base = stub_cms(Some(serde_json::json!({
            "instruments": ["BTCUSD"],
            "demoStartBalanceCents": 77_700
        })))
        .await;
        let st = with_cms(build_state_with(tenanted(DEFAULT_USER)), base);
        restore_state(&st).await.unwrap();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        assert_eq!(post_json(&app, "/deals", deal()).await.0, StatusCode::OK);

        let all = queued(&st).await;
        let names: Vec<String> = all
            .iter()
            .map(|e| e["event"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == bus::EV_ACCOUNT_OPENED),
            "счёт заведён, событие обязано быть: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == bus::EV_BALANCE_CHANGED),
            "заведение — не изменение остатка: {names:?}"
        );
    }

    #[tokio::test]
    async fn an_unreadable_identity_refuses_instead_of_falling_back() {
        let st = build_state_with(Arc::new(BlindIdentity(persistence::MemoryStore::new())));
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        let (status, body) = post_json(&app, "/deals", deal()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(body["error"], "identity_store_unavailable", "{body}");

        let (_, acc) = get_json(&app, "/account").await;
        assert_eq!(
            acc["opened"],
            serde_json::json!(false),
            "«не смогли прочитать» — не повод завести счёт своей суммой: {acc}"
        );
    }

    fn tenanted(linked: UserId) -> Arc<Tenanted> {
        Arc::new(Tenanted {
            inner: persistence::MemoryStore::new(),
            linked,
        })
    }

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> StatusCode {
        let b = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let _ = resp.into_body().collect().await.unwrap();
        status
    }

    async fn with_price(st: &AppState, instrument: u32, px: i64) {
        st.tickers.lock().await.insert(
            instrument,
            feed::Ticker {
                last: px,
                change: 0.0,
                high: px,
                bid: px,
                ask: px,
                ..Default::default()
            },
        );
    }

    async fn queued(st: &AppState) -> Vec<serde_json::Value> {
        st.store
            .claim_outbox(100, 0)
            .await
            .expect("очередь обязана читаться")
            .iter()
            .map(|c| serde_json::from_str(&c.payload).expect("конверт — валидный JSON"))
            .collect()
    }

    fn of_kind<'a>(all: &'a [serde_json::Value], name: &str) -> Vec<&'a serde_json::Value> {
        all.iter().filter(|e| e["event"] == name).collect()
    }

    #[tokio::test]
    async fn a_stop_loss_reaches_the_outgoing_queue() {
        let st = build_state_with(tenanted(DEFAULT_USER));
        restore_state(&st).await.unwrap();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        let body = serde_json::json!({
            "instrument": 1, "side": "buy", "qty": 100_000, "sl": 4_900_000
        });
        assert_eq!(
            send(&app, "POST", "/deals", Some(body)).await,
            StatusCode::OK
        );

        with_price(&st, 1, 4_800_000).await;
        monitor_tick(&st).await;

        let all = queued(&st).await;
        let trades = of_kind(&all, bus::EV_TRADE_EXECUTED);
        assert_eq!(trades.len(), 1, "сделка обязана уйти наружу: {all:?}");
        let d = &trades[0]["data"];
        assert_eq!(d["tenant"], TENANT);
        assert_eq!(d["userId"], SUB);
        assert_eq!(d["symbol"], "BTCUSD");
        assert_eq!(d["price"], serde_json::json!(48_000.0), "цена ИСПОЛНЕНИЯ");
        assert_eq!(
            d["level"],
            serde_json::json!(49_000.0),
            "и поставленный уровень — без него проскальзывание не посчитать"
        );
        assert_eq!(d["acrossGap"], serde_json::json!(false));
        assert_eq!(trades[0]["source"], bus::SOURCE);
        assert_eq!(trades[0]["version"], bus::EVENT_VERSION);

        let balances = of_kind(&all, bus::EV_BALANCE_CHANGED);
        assert_eq!(balances.len(), 1, "изменение остатка обязано уехать");
        assert_eq!(balances[0]["data"]["reason"], "trade");
    }

    #[tokio::test]
    async fn opening_an_account_reaches_the_queue_before_the_trade() {
        let st = build_state_with(tenanted(DEFAULT_USER));
        restore_state(&st).await.unwrap();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        assert_eq!(
            send(
                &app,
                "POST",
                "/deals",
                Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 100_000}))
            )
            .await,
            StatusCode::OK
        );

        let all = queued(&st).await;
        assert_eq!(
            all.first().map(|e| e["event"].clone()),
            Some(serde_json::json!(bus::EV_ACCOUNT_OPENED)),
            "счёт заводится раньше, чем по нему торгуют: {all:?}"
        );
    }

    #[tokio::test]
    async fn a_failed_write_publishes_nothing() {
        struct Burning(persistence::MemoryStore);
        #[async_trait::async_trait]
        impl Persistence for Burning {
            async fn init(&self) -> Result<(), persistence::StoreError> {
                Ok(())
            }
            async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
                Ok(Default::default())
            }
            async fn save_account(
                &self,
                _u: UserId,
                _s: &broker::AccountSnapshot,
                _e: &[journal::AccountEvent],
                _o: &[persistence::OutboxRecord],
            ) -> Result<(), persistence::StoreError> {
                Err(persistence::StoreError("диск горит".into()))
            }
            async fn claim_outbox(
                &self,
                limit: i64,
                lease_ms: i64,
            ) -> Result<Vec<persistence::OutboxClaim>, persistence::StoreError> {
                self.0.claim_outbox(limit, lease_ms).await
            }
            async fn outbox_backlog(&self) -> Result<i64, persistence::StoreError> {
                self.0.outbox_backlog().await
            }
            async fn identity_of(
                &self,
                _u: UserId,
            ) -> Result<Option<(String, String)>, persistence::StoreError> {
                Ok(Some((TENANT.to_string(), SUB.to_string())))
            }
        }

        let st = build_state_with(Arc::new(Burning(persistence::MemoryStore::new())));
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        assert_eq!(
            send(
                &app,
                "POST",
                "/deals",
                Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 100_000}))
            )
            .await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            st.store.outbox_backlog().await.unwrap(),
            0,
            "события не существует отдельно от денег"
        );
    }

    #[tokio::test]
    async fn an_unlinked_account_publishes_nothing() {
        let st = build_state_with(tenanted(UserId(999)));
        restore_state(&st).await.unwrap();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        assert_eq!(
            send(
                &app,
                "POST",
                "/deals",
                Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 100_000}))
            )
            .await,
            StatusCode::OK,
            "торговля не зависит от того, доедет ли событие"
        );
        assert_eq!(st.store.outbox_backlog().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_margin_call_reaches_the_outgoing_queue() {
        let st = build_state_with(tenanted(DEFAULT_USER));
        restore_state(&st).await.unwrap();
        {
            let mut b = st.broker.lock().await;
            *b = broker::Broker::new(100_000, 10)
                .with_margin_call_pct(Some(100))
                .with_stop_out_pct(Some(50));
        }
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        assert_eq!(
            send(
                &app,
                "POST",
                "/deals",
                Some(serde_json::json!({"instrument":1,"side":"buy","qty":10_000}))
            )
            .await,
            StatusCode::OK
        );

        with_price(&st, 1, 4_500_000).await;
        monitor_tick(&st).await;

        let all = queued(&st).await;
        let calls = of_kind(&all, bus::EV_MARGIN_CALLED);
        assert_eq!(calls.len(), 1, "предупреждение обязано уехать: {all:?}");
        let d = &calls[0]["data"];
        assert_eq!(d["tenant"], TENANT);
        assert_eq!(d["userId"], SUB);
        assert_eq!(
            d["stopOutLevelPct"], 50,
            "в предупреждении обязан быть и порог закрытия: без него не видно, сколько осталось"
        );
        assert_eq!(
            d["marginLevelPct"], 100,
            "достигнутый уровень — ровно порог: {d}"
        );
        assert_eq!(d["usedMarginCents"], 50_000, "{d}");
        assert_eq!(d["equityCents"], 50_000, "{d}");
        assert_eq!(calls[0]["event"], "terminal.margin.called");

        let seq = d["accountSeq"]
            .as_u64()
            .expect("позиция в журнале обязана быть");
        assert!(seq > 0, "ноль — не позиция в журнале счёта с историей: {d}");
        let last = {
            let js = st.journals.lock().await;
            js.get(&DEFAULT_USER)
                .and_then(|j| j.events().last())
                .map(|e| e.seq())
                .expect("журнал счёта не пуст")
        };
        assert_eq!(
            seq, last,
            "номер обязан совпасть с последней записью журнала: {d}"
        );

        assert!(
            of_kind(&all, bus::EV_TRADE_EXECUTED).is_empty(),
            "на уровне предупреждения ещё не закрывают: {all:?}"
        );
    }

    #[tokio::test]
    async fn a_demo_reset_moves_the_money_and_tells_the_bus() {
        let st = build_state_with(tenanted(DEFAULT_USER));
        restore_state(&st).await.unwrap();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        assert_eq!(
            send(
                &app,
                "POST",
                "/deals",
                Some(serde_json::json!({"instrument":1,"side":"buy","qty":1000}))
            )
            .await,
            StatusCode::OK
        );
        with_price(&st, 1, 4_000_000).await;
        assert_eq!(
            send(&app, "POST", "/deals/1/close", None).await,
            StatusCode::OK
        );
        let lost = st
            .broker
            .lock()
            .await
            .balance(DEFAULT_USER)
            .expect("счёт заведён");
        assert!(lost < 10_000_000, "счёт обязан просесть");

        assert_eq!(
            send(&app, "POST", "/account/demo-reset", None).await,
            StatusCode::OK
        );
        assert_eq!(
            st.broker.lock().await.balance(DEFAULT_USER),
            Some(10_000_000),
            "деньги меняет ТЕРМИНАЛ, а не кабинет у себя"
        );

        let all = queued(&st).await;
        let last = of_kind(&all, bus::EV_BALANCE_CHANGED)
            .last()
            .copied()
            .expect("событие об остатке обязано уехать");
        assert_eq!(last["data"]["balanceCents"], 10_000_000);
        assert_eq!(last["data"]["reason"], "reset");

        let kinds: Vec<String> = st
            .journals
            .lock()
            .await
            .get(&DEFAULT_USER)
            .map(|j| j.events().iter().map(|e| e.kind().encode()).collect())
            .unwrap_or_default();
        assert!(
            kinds
                .iter()
                .any(|k| k.starts_with("demo_account_reset adjustment=")),
            "сброс обязан быть именованной проводкой: {kinds:?}"
        );
    }

    #[tokio::test]
    async fn health_tells_whether_the_bus_is_configured_and_how_much_waits() {
        let st = build_state_with(tenanted(DEFAULT_USER));
        restore_state(&st).await.unwrap();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        let _ = send(
            &app,
            "POST",
            "/deals",
            Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 100_000})),
        )
        .await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["bus"], false, "в тестах шина не настроена, и это видно");
        assert_eq!(
            v["outbox_pending"], 1,
            "и видно, сколько ждёт отправки: {v}"
        );
        assert_eq!(
            v["shared_account"], true,
            "общий счёт обязан быть виден машине, а не только по симптому: {v}"
        );
    }
}

#[cfg(test)]
mod margin_wiring_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get(app: &Router, uri: &str) -> serde_json::Value {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    async fn post(app: &Router, uri: &str, body: serde_json::Value) -> StatusCode {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        app.clone().oneshot(req).await.unwrap().status()
    }

    async fn price(st: &AppState, instrument: u32, px: i64) {
        st.tickers.lock().await.insert(
            instrument,
            feed::Ticker {
                last: px,
                change: 0.0,
                high: px,
                bid: px,
                ask: px,
                ..Default::default()
            },
        );
    }

    #[tokio::test]
    async fn an_account_that_does_not_exist_yet_is_named_not_invented() {
        let st = build_state();
        let app = router(st.clone());

        let acc = get(&app, "/account").await;

        assert_eq!(acc["opened"], serde_json::json!(false), "{acc}");
        for f in [
            "balance",
            "equity",
            "used_margin",
            "free_margin",
            "open_pnl",
            "model",
        ] {
            assert!(
                acc[f].is_null(),
                "{f} у незаведённого счёта обязан быть null, а не числом: {acc}"
            );
        }
        assert_ne!(acc["balance"], serde_json::json!(0), "{acc}");

        assert!(
            acc.get("negative_balance_protection").is_some(),
            "защита остатка — свойство площадки, она известна: {acc}"
        );

        price(&st, 1, 5_000_000).await;
        assert_eq!(
            post(
                &app,
                "/deals",
                serde_json::json!({"instrument":1,"side":"buy","qty":1000})
            )
            .await,
            StatusCode::OK
        );
        let acc = get(&app, "/account").await;
        assert_eq!(acc["opened"], serde_json::json!(true), "{acc}");
        assert!(acc["balance"].is_i64(), "{acc}");
    }

    #[tokio::test]
    async fn the_screen_and_the_engine_agree_on_free_margin() {
        let st = build_state();
        price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        assert_eq!(
            post(
                &app,
                "/deals",
                serde_json::json!({"instrument":1,"side":"buy","qty":1000})
            )
            .await,
            StatusCode::OK
        );

        price(&st, 1, 2_500_000).await;

        let acc = get(&app, "/account").await;
        let free = acc["free_margin"].as_i64().expect("свободная маржа");
        assert!(
            free < acc["balance"].as_i64().unwrap() - acc["used_margin"].as_i64().unwrap(),
            "экран обязан учитывать нереализованный убыток: {acc}"
        );

        const PX: i64 = 2_500_000;
        let too_much = free * 100_000 / PX + 10_000;
        let status = post(
            &app,
            "/deals",
            serde_json::json!({"instrument":1,"side":"buy","qty": too_much}),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::PAYMENT_REQUIRED,
            "движок обязан отказать там же, где отказывает экран: free={free}, {acc}"
        );
    }

    #[tokio::test]
    async fn margin_level_is_served_and_undefined_without_positions() {
        let st = build_state();
        price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        assert_eq!(
            post(
                &app,
                "/deals",
                serde_json::json!({"instrument":1,"side":"buy","qty":1000})
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            post(&app, "/deals/1/close", serde_json::json!({})).await,
            StatusCode::OK
        );

        let acc = get(&app, "/account").await;
        assert_eq!(acc["used_margin"], 0, "позиций не осталось: {acc}");
        assert!(
            acc.get("margin_level_pct").is_none(),
            "позиций нет — уровня нет, а не ноль: {acc}"
        );
        assert!(
            acc.get("stop_out_level_pct").is_none(),
            "порог в тестах не задан, и это видно: {acc}"
        );

        assert_eq!(
            post(
                &app,
                "/deals",
                serde_json::json!({"instrument":1,"side":"buy","qty":1000})
            )
            .await,
            StatusCode::OK
        );
        let acc = get(&app, "/account").await;
        assert_eq!(
            acc["used_margin"], 50_000,
            "нотионал 0.01 × $50 000 = $500: {acc}"
        );
        assert_eq!(acc["margin_level_pct"], 20_000, "{acc}");
    }
}

#[cfg(test)]
mod leverage_env_tests {
    use super::leverage_from_env;

    #[test]
    fn leverage_defaults_to_one_and_refuses_nonsense() {
        assert_eq!(
            leverage_from_env(None),
            1,
            "без переменной поведение прежнее"
        );
        assert_eq!(leverage_from_env(Some("")), 1);
        assert_eq!(leverage_from_env(Some(" 20 ")), 20);
        assert_eq!(leverage_from_env(Some("1000")), 1000);
        for bad in ["0", "-5", "1001", "20x", "1.5"] {
            assert_eq!(leverage_from_env(Some(bad)), 1, "«{bad}»");
        }
    }
}
