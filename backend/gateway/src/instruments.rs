use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;

use crate::dec::Dec;
use crate::feed::{self, FeedInstrument, Source};
use crate::{money, normalize_symbol, symbol_index, AppState};

pub const POLL_SECS: u64 = 30;
pub const CATALOG_REFRESH_SECS: u64 = 24 * 3600;
pub const CATALOG_RETRY_SECS: u64 = 600;
pub const QUOTE_STALE_MS: i64 = 20_000;
pub const V2_SILENCE_RECONNECT_SECS: u64 = 45;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[serde(rename = "trade")]
    Trade,
    #[serde(rename = "closeOnly")]
    CloseOnly,
}

impl Mode {
    pub fn tag(self) -> &'static str {
        match self {
            Mode::Trade => "trade",
            Mode::CloseOnly => "closeOnly",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformEntry {
    pub symbol: String,
    pub mode: Mode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformList {
    pub version: i64,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: Option<String>,
    pub instruments: Vec<PlatformEntry>,
}

pub fn parse_platform_list(v: &Value) -> Result<PlatformList, String> {
    let version = v
        .get("version")
        .and_then(Value::as_i64)
        .filter(|n| *n >= 0)
        .ok_or("нет version")?;
    let updated_at = v
        .get("updatedAt")
        .and_then(Value::as_str)
        .map(str::to_string);
    let raw = v
        .get("instruments")
        .and_then(Value::as_array)
        .ok_or("нет instruments")?;
    let mut seen = HashSet::new();
    let mut instruments = Vec::new();
    for item in raw {
        let symbol = item
            .get("symbol")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty() && s.len() <= 32)
            .ok_or("элемент без symbol")?;
        let mode = match item.get("mode").and_then(Value::as_str) {
            Some("trade") => Mode::Trade,
            Some("closeOnly") => Mode::CloseOnly,
            other => return Err(format!("{symbol}: неизвестный mode {other:?}")),
        };
        if seen.insert(symbol.clone()) {
            instruments.push(PlatformEntry { symbol, mode });
        }
    }
    if instruments.is_empty() {
        return Err(format!(
            "пустой список инструментов (version {version}) — не применяется"
        ));
    }
    Ok(PlatformList {
        version,
        updated_at,
        instruments,
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub symbol: String,
    pub source: String,
    pub provider_symbol: String,
    pub category: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub price_decimals: Option<u8>,
    pub tick_size: String,
    pub qty_step: String,
    #[serde(default)]
    pub min_qty: Option<String>,
    #[serde(default)]
    pub contract_size: Option<String>,
    #[serde(default)]
    pub qty_decimals: Option<u8>,
    pub has_depth: bool,
    pub has_trades: bool,
    pub has_1s: bool,
    #[serde(default)]
    pub session: Option<Value>,
    #[serde(default)]
    pub status: Option<String>,
}

fn filter_value<'a>(filters: &'a [Value], kind: &str, key: &str) -> Option<&'a str> {
    filters
        .iter()
        .find(|f| f.get("filterType").and_then(Value::as_str) == Some(kind))
        .and_then(|f| f.get(key))
        .and_then(Value::as_str)
}

pub fn parse_exchange_info(v: &Value) -> (Vec<CatalogEntry>, Vec<String>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let Some(list) = v.get("symbols").and_then(Value::as_array) else {
        return (out, vec!["(ответ без symbols[])".into()]);
    };
    for s in list {
        let str_of = |k: &str| s.get(k).and_then(Value::as_str);
        let (Some(symbol), Some(base), Some(quote)) =
            (str_of("symbol"), str_of("baseAsset"), str_of("quoteAsset"))
        else {
            continue;
        };
        if quote != "USDT" || str_of("status").is_some_and(|st| st != "TRADING") {
            continue;
        }
        let filters = s
            .get("filters")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let tick = filter_value(filters, "PRICE_FILTER", "tickSize")
            .and_then(Dec::parse)
            .filter(|d| d.is_positive());
        let step = filter_value(filters, "LOT_SIZE", "stepSize")
            .and_then(Dec::parse)
            .filter(|d| d.is_positive());
        let (Some(tick), Some(step)) = (tick, step) else {
            skipped.push(symbol.to_string());
            continue;
        };
        let min_qty = filter_value(filters, "LOT_SIZE", "minQty")
            .and_then(Dec::parse)
            .filter(|d| d.is_positive())
            .map(|d| d.to_string());
        out.push(CatalogEntry {
            symbol: format!("{}USD", base.to_uppercase()),
            source: Source::V1.tag().into(),
            provider_symbol: symbol.to_string(),
            category: "crypto".into(),
            name: None,
            price_decimals: None,
            tick_size: tick.to_string(),
            qty_step: step.to_string(),
            min_qty,
            contract_size: None,
            qty_decimals: None,
            has_depth: true,
            has_trades: true,
            has_1s: true,
            session: None,
            status: None,
        });
    }
    (out, skipped)
}

const V2_CATEGORIES: [&str; 5] = ["forex", "metals", "indices", "stocks", "energy"];

pub fn parse_v2_instruments(v: &Value) -> (Vec<CatalogEntry>, Vec<String>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let Some(list) = v.get("instruments").and_then(Value::as_array) else {
        return (out, vec!["(ответ без instruments[])".into()]);
    };
    for i in list {
        let str_of = |k: &str| i.get(k).and_then(Value::as_str);
        let Some(symbol) = str_of("symbol").map(str::to_uppercase) else {
            continue;
        };
        let Some(category) = str_of("category").filter(|c| V2_CATEGORIES.contains(c)) else {
            if str_of("category") != Some("crypto") {
                skipped.push(symbol);
            }
            continue;
        };
        let dec = |k: &str| str_of(k).and_then(Dec::parse).filter(|d| d.is_positive());
        let (Some(tick), Some(step)) = (dec("tickSize"), dec("qtyStep")) else {
            skipped.push(symbol);
            continue;
        };
        let small = |k: &str| {
            i.get(k)
                .and_then(Value::as_u64)
                .and_then(|n| u8::try_from(n).ok())
        };
        out.push(CatalogEntry {
            symbol: symbol.clone(),
            source: Source::V2.tag().into(),
            provider_symbol: symbol,
            category: category.to_string(),
            name: str_of("name").map(str::to_string),
            price_decimals: small("priceDecimals"),
            tick_size: tick.to_string(),
            qty_step: step.to_string(),
            min_qty: dec("minQty").map(|d| d.to_string()),
            contract_size: Some(
                dec("contractSize")
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "1".into()),
            ),
            qty_decimals: small("qtyDecimals"),
            has_depth: i.get("hasDepth").and_then(Value::as_bool).unwrap_or(false),
            has_trades: i.get("hasTrades").and_then(Value::as_bool).unwrap_or(false),
            has_1s: i.get("has1s").and_then(Value::as_bool).unwrap_or(false),
            session: i.get("session").filter(|s| s.is_object()).cloned(),
            status: str_of("status").map(str::to_string),
        });
    }
    (out, skipped)
}

const MAX_DECIMALS: u32 = 12;

fn contract_of(e: &CatalogEntry) -> Dec {
    e.contract_size
        .as_deref()
        .and_then(Dec::parse)
        .filter(|d| d.is_positive())
        .unwrap_or(Dec { m: 1, s: 0 })
}

pub fn storage_precision(e: &CatalogEntry) -> Option<(u8, u8)> {
    let tick = Dec::parse(&e.tick_size)?;
    let step = Dec::parse(&e.qty_step)?;
    let contract = contract_of(e);
    let pd = tick
        .decimals()
        .max(u32::from(e.price_decimals.unwrap_or(0)));
    let mut qd = step.mul(contract)?.decimals();
    if let Some(min) = e.min_qty.as_deref().and_then(Dec::parse) {
        qd = qd.max(min.mul(contract)?.decimals());
    }
    if pd > MAX_DECIMALS || qd > MAX_DECIMALS {
        return None;
    }
    Some((pd as u8, qd as u8))
}

pub fn display_symbol(source: Source, provider: &str, canonical: &str) -> String {
    match source {
        Source::V1 => match provider.strip_suffix("USDT") {
            Some(base) if !base.is_empty() => format!("{base}-USDT"),
            _ => provider.to_string(),
        },
        Source::V2 => canonical.to_string(),
    }
}

pub fn instrument_from(
    row: &persistence::InstrumentRow,
    entry: Option<&CatalogEntry>,
    legacy: Option<&FeedInstrument>,
) -> FeedInstrument {
    let source = Source::parse(&row.source).unwrap_or(Source::V1);
    let pd = row.price_decimals;
    let qd = row.qty_decimals;
    let mut f = match legacy {
        Some(l) => l.clone(),
        None => FeedInstrument {
            id: row.id,
            symbol: display_symbol(source, &row.provider_symbol, &row.symbol),
            binance: row.provider_symbol.clone(),
            base: row.id,
            quote: 100,
            price_decimals: pd,
            qty_decimals: qd,
            canonical: row.symbol.clone(),
            source,
            category: if source == Source::V1 {
                "crypto".into()
            } else {
                "other".into()
            },
            has_depth: source == Source::V1,
            has_trades: source == Source::V1,
            has_1s: source == Source::V1,
            ..FeedInstrument::default()
        },
    };
    let Some(e) = entry.filter(|e| e.source == source.tag()) else {
        return f;
    };
    let contract = contract_of(e);
    f.tick_size = Dec::parse(&e.tick_size).and_then(|d| d.raw(u32::from(pd)));
    f.qty_step = Dec::parse(&e.qty_step)
        .and_then(|d| d.mul(contract))
        .and_then(|d| d.raw(u32::from(qd)));
    f.min_qty = e
        .min_qty
        .as_deref()
        .and_then(Dec::parse)
        .and_then(|d| d.mul(contract))
        .and_then(|d| d.raw(u32::from(qd)));
    if source == Source::V2 {
        f.category = e.category.clone();
        f.name = e.name.clone();
        f.contract_size = Some(contract.to_string());
        f.lot_decimals = e.qty_decimals;
        f.has_depth = e.has_depth;
        f.has_trades = e.has_trades;
        f.has_1s = e.has_1s;
        f.session = e.session.clone();
    }
    f
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub by_id: BTreeMap<u32, FeedInstrument>,
    pub platform: Vec<(u32, Mode)>,
    pub source: &'static str,
    pub version: Option<i64>,
    pub updated_at: Option<String>,
}

impl Catalog {
    pub fn legacy() -> Catalog {
        let list = feed::instruments();
        Catalog {
            platform: list.iter().map(|f| (f.id, Mode::Trade)).collect(),
            by_id: list.into_iter().map(|f| (f.id, f)).collect(),
            source: "builtin",
            version: None,
            updated_at: None,
        }
    }

    pub fn get(&self, id: u32) -> Option<&FeedInstrument> {
        self.by_id.get(&id)
    }

    pub fn platform_mode(&self, id: u32) -> Mode {
        self.platform
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, m)| *m)
            .unwrap_or(Mode::CloseOnly)
    }

    pub fn index(&self) -> HashMap<String, u32> {
        let all: Vec<FeedInstrument> = self.by_id.values().cloned().collect();
        symbol_index(&all)
    }

    pub fn id_of(&self, canonical: &str) -> Option<u32> {
        self.by_id
            .values()
            .find(|f| f.canonical == canonical)
            .map(|f| f.id)
    }

    pub fn shown(&self, extra: &BTreeSet<u32>) -> Vec<FeedInstrument> {
        let mut out: Vec<FeedInstrument> = self
            .platform
            .iter()
            .filter_map(|(id, _)| self.by_id.get(id).cloned())
            .collect();
        for id in extra {
            if !self.platform.iter().any(|(i, _)| i == id) {
                if let Some(f) = self.by_id.get(id) {
                    out.push(f.clone());
                }
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Open,
    Closed,
    Halted,
}

impl Status {
    pub fn tag(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Closed => "closed",
            Status::Halted => "halted",
        }
    }

    pub fn parse(t: &str) -> Option<Status> {
        match t {
            "open" => Some(Status::Open),
            "closed" => Some(Status::Closed),
            "halted" => Some(Status::Halted),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketState {
    pub status: Status,
    pub next_open: Option<i64>,
    pub reason: Option<String>,
    pub last_quote_ms: i64,
    pub gap: bool,
}

impl MarketState {
    pub fn new(status: Status) -> Self {
        MarketState {
            status,
            next_open: None,
            reason: None,
            last_quote_ms: 0,
            gap: true,
        }
    }

    pub fn tradable(&self, now_ms: i64) -> bool {
        self.status == Status::Open
            && self.last_quote_ms > 0
            && now_ms - self.last_quote_ms <= QUOTE_STALE_MS
    }
}

#[derive(Clone)]
pub struct CmsEndpoint {
    pub base: String,
    pub api_key: String,
}

#[derive(Clone)]
pub struct V2Endpoint {
    pub rest: String,
    pub ws: String,
}

pub struct Platform {
    pub cms: Option<CmsEndpoint>,
    pub webhook_secret: Option<String>,
    pub cache_path: Option<PathBuf>,
    pub v1_rest: String,
    pub v2: Option<V2Endpoint>,
    pub reject_close_only: bool,
    pub http: reqwest::Client,
    entries: std::sync::Mutex<HashMap<String, CatalogEntry>>,
    entries_at: std::sync::Mutex<Option<Instant>>,
    apply: tokio::sync::Mutex<()>,
    subs_v1: watch::Sender<BTreeSet<String>>,
    subs_v2: watch::Sender<BTreeSet<String>>,
    site_close_only: std::sync::Mutex<HashMap<String, HashSet<u32>>>,
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub fn v2_ws_from(api: &str, ws: Option<&str>) -> String {
    let base = match ws.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => w.trim_end_matches('/').to_string(),
        None => {
            let api = api.trim_end_matches('/');
            if let Some(rest) = api.strip_prefix("https://") {
                format!("wss://{rest}")
            } else if let Some(rest) = api.strip_prefix("http://") {
                format!("ws://{rest}")
            } else {
                api.to_string()
            }
        }
    };
    if base.ends_with("/v2/stream") {
        base
    } else {
        format!("{base}/v2/stream")
    }
}

impl Platform {
    pub fn new(
        cms: Option<CmsEndpoint>,
        v1_rest: String,
        v2: Option<V2Endpoint>,
        cache_path: Option<PathBuf>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Platform {
            cms,
            webhook_secret: None,
            cache_path,
            v1_rest,
            v2,
            reject_close_only: false,
            http,
            entries: std::sync::Mutex::new(HashMap::new()),
            entries_at: std::sync::Mutex::new(None),
            apply: tokio::sync::Mutex::new(()),
            subs_v1: watch::channel(BTreeSet::new()).0,
            subs_v2: watch::channel(BTreeSet::new()).0,
            site_close_only: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn from_env() -> Self {
        let cms = match (env_nonempty("CMS_BASE_URL"), env_nonempty("CMS_API_KEY")) {
            (Some(base), Some(api_key)) => Some(CmsEndpoint {
                base: base.trim_end_matches('/').to_string(),
                api_key,
            }),
            _ => None,
        };
        let v2 = env_nonempty("MARKET_V2_API_URL").map(|api| V2Endpoint {
            ws: v2_ws_from(&api, env_nonempty("MARKET_V2_WS_URL").as_deref()),
            rest: api.trim_end_matches('/').to_string(),
        });
        let cache_path = Some(PathBuf::from(
            env_nonempty("PLATFORM_INSTRUMENTS_CACHE")
                .unwrap_or_else(|| "data/platform-instruments.json".into()),
        ));
        let mut p = Platform::new(cms, feed::rest_base().to_string(), v2, cache_path);
        p.webhook_secret = env_nonempty("CMS_WEBHOOK_SECRET");
        p.reject_close_only = matches!(
            env_nonempty("CLOSE_ONLY_REJECT_PENDING_ENABLED").as_deref(),
            Some("1") | Some("true")
        );
        match &p.cms {
            Some(c) => println!(
                "[instruments] список платформы: CMS {}/cms/platform/instruments, опрос {POLL_SECS} с, вебхук {}",
                c.base,
                if p.webhook_secret.is_some() { "включён" } else { "выключен (CMS_WEBHOOK_SECRET не задан)" }
            ),
            None => println!("[instruments] список платформы: CMS не сконфигурирована — сохранённый список или встроенные 30 пар"),
        }
        match &p.v2 {
            Some(v) => println!(
                "[instruments] модуль рыночных данных v2: REST {}, WS {}",
                feed::masked(&v.rest),
                feed::masked(&v.ws)
            ),
            None => println!("[instruments] модуль рыночных данных v2 не задан (MARKET_V2_API_URL) — форекс, металлы, акции не подключаются"),
        }
        if p.reject_close_only {
            println!("[instruments] отложенные ордера по инструменту «только закрытие» при срабатывании СНИМАЮТСЯ записью pending_rejected (CLOSE_ONLY_REJECT_PENDING_ENABLED=1)");
        } else {
            println!("[instruments] отложенные ордера по инструменту «только закрытие» ждут (CLOSE_ONLY_REJECT_PENDING_ENABLED не 1); запись pending_rejected reason=instrument_close_only при этом читается");
        }
        p
    }

    pub fn subscriptions_v1(&self) -> watch::Receiver<BTreeSet<String>> {
        self.subs_v1.subscribe()
    }

    pub fn subscriptions_v2(&self) -> watch::Receiver<BTreeSet<String>> {
        self.subs_v2.subscribe()
    }

    pub fn entry(&self, symbol: &str) -> Option<CatalogEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(symbol)
            .cloned()
    }

    fn merge_entries(&self, list: Vec<CatalogEntry>) {
        let mut m = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        for e in list {
            let v2 = e.source == Source::V2.tag();
            match m.get(&e.symbol) {
                Some(old) if old.source == Source::V2.tag() && !v2 => {}
                _ => {
                    m.insert(e.symbol.clone(), e);
                }
            }
        }
    }

    pub fn note_site_close_only(&self, site: &str, ids: HashSet<u32>) {
        self.site_close_only
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(site.to_string(), ids);
    }

    pub fn site_close_only_snapshot(&self) -> HashMap<String, HashSet<u32>> {
        self.site_close_only
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    #[serde(rename = "savedAt")]
    saved_at: i64,
    list: PlatformList,
    #[serde(default)]
    catalog: Vec<CatalogEntry>,
}

fn load_cache(path: &std::path::Path) -> Result<Option<CacheFile>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let file: CacheFile = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let v = serde_json::to_value(&file.list).map_err(|e| e.to_string())?;
    parse_platform_list(&v)?;
    Ok(Some(file))
}

fn save_cache(path: &std::path::Path, file: &CacheFile) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(file).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn verify_signature(body: &[u8], header: Option<&str>, secret: &str) -> bool {
    use hmac::{Hmac, Mac};
    let Some(hex) = header.and_then(|h| h.strip_prefix("sha256=")) else {
        return false;
    };
    let Some(given) = decode_hex(hex) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&given).is_ok()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

pub fn sign(body: &[u8], secret: &str) -> String {
    use hmac::{Hmac, Mac};
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC принимает любой ключ");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

pub async fn load_registry(st: &AppState) -> Result<(), persistence::StoreError> {
    let legacy = feed::legacy_rows();
    st.store.pin_instruments(&legacy).await?;
    let rows = st.store.load_instruments().await?;
    for l in &legacy {
        let by_id = rows.iter().find(|r| r.id == l.id);
        let by_symbol = rows.iter().find(|r| r.symbol == l.symbol);
        let bad = by_id.is_some_and(|r| r != l) || by_symbol.is_some_and(|r| r != l);
        if bad {
            return Err(persistence::StoreError(format!(
                "реестр инструментов расходится со встроенным для id {} ({}): в базе {:?} / {:?}. Цены и объёмы журнала читались бы в чужой точности — старт отклонён",
                l.id, l.symbol, by_id, by_symbol
            )));
        }
    }
    let cached = match st.platform.cache_path.as_deref() {
        Some(path) => match load_cache(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "[instruments] сохранённый список {} не читается ({e}) — не используется",
                    path.display()
                );
                None
            }
        },
        None => None,
    };
    if let Some(c) = &cached {
        st.platform.merge_entries(c.catalog.clone());
    }
    let legacy_meta: HashMap<u32, FeedInstrument> =
        feed::instruments().into_iter().map(|f| (f.id, f)).collect();
    let mut catalog = Catalog::legacy();
    for r in &rows {
        let entry = st.platform.entry(&r.symbol);
        catalog.by_id.insert(
            r.id,
            instrument_from(r, entry.as_ref(), legacy_meta.get(&r.id)),
        );
    }
    let registered = rows.len().max(legacy.len());
    match cached {
        Some(c) => {
            catalog.platform = resolve_platform(&catalog, &c.list);
            catalog.source = "file";
            catalog.version = Some(c.list.version);
            catalog.updated_at = c.list.updated_at.clone();
            println!(
                "[instruments] список платформы из сохранённого файла: version {}, инструментов {} (зарегистрировано {registered})",
                c.list.version,
                catalog.platform.len()
            );
        }
        None => eprintln!(
            "[instruments] ни CMS, ни сохранённого списка ещё нет — работаем по встроенному списку из {} пар",
            catalog.platform.len()
        ),
    }
    for f in catalog.by_id.values().filter(|f| f.source == Source::V2) {
        let status = st
            .platform
            .entry(&f.canonical)
            .and_then(|e| e.status)
            .and_then(|s| Status::parse(&s))
            .unwrap_or(Status::Closed);
        st.markets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(f.id)
            .or_insert_with(|| MarketState::new(status));
    }
    *st.catalog.write().unwrap_or_else(|e| e.into_inner()) = catalog;
    recompute_subscriptions(st).await;
    Ok(())
}

fn resolve_platform(catalog: &Catalog, list: &PlatformList) -> Vec<(u32, Mode)> {
    list.instruments
        .iter()
        .filter_map(|e| catalog.id_of(&e.symbol).map(|id| (id, e.mode)))
        .collect()
}

async fn fetch_value(http: &reqwest::Client, url: &str) -> Result<Value, String> {
    let resp = http.get(url).send().await.map_err(|e| {
        let text = e.to_string();
        text.replace(url, &feed::masked(url))
    })?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

pub async fn refresh_entries(st: &AppState, force: bool) -> bool {
    {
        let at = st
            .platform
            .entries_at
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !force && at.is_some_and(|t| t.elapsed() < Duration::from_secs(CATALOG_RETRY_SECS)) {
            return false;
        }
    }
    *st.platform
        .entries_at
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    let p = &st.platform;
    let mut got = false;
    let v1_url = format!("{}/api/v3/exchangeInfo", p.v1_rest);
    match fetch_value(&p.http, &v1_url).await {
        Ok(v) => {
            let (list, skipped) = parse_exchange_info(&v);
            if !skipped.is_empty() {
                eprintln!(
                    "[instruments] каталог v1: без PRICE_FILTER/LOT_SIZE пропущено {} пар",
                    skipped.len()
                );
            }
            println!("[instruments] каталог v1 (крипто): {} пар", list.len());
            got |= !list.is_empty();
            p.merge_entries(list);
        }
        Err(e) => eprintln!("[instruments] каталог v1 не получен: {e}"),
    }
    if let Some(v2) = &p.v2 {
        match fetch_value(&p.http, &format!("{}/v2/instruments", v2.rest)).await {
            Ok(v) => {
                let (list, skipped) = parse_v2_instruments(&v);
                if !skipped.is_empty() {
                    eprintln!(
                        "[instruments] каталог v2: пропущены [{}]",
                        skipped.join(", ")
                    );
                }
                println!("[instruments] каталог v2: {} инструментов", list.len());
                got |= !list.is_empty();
                p.merge_entries(list);
            }
            Err(e) => eprintln!(
                "[instruments] каталог v2 не получен: {}",
                e.replace(&v2.rest, &feed::masked(&v2.rest))
            ),
        }
    }
    if got {
        rebuild_metadata(st);
    }
    got
}

fn rebuild_metadata(st: &AppState) {
    let mut cat = st.catalog.write().unwrap_or_else(|e| e.into_inner());
    let legacy: HashMap<u32, FeedInstrument> =
        feed::instruments().into_iter().map(|f| (f.id, f)).collect();
    for f in cat.by_id.values_mut() {
        let row = persistence::InstrumentRow {
            id: f.id,
            symbol: f.canonical.clone(),
            source: f.source.tag().into(),
            provider_symbol: f.binance.clone(),
            price_decimals: f.price_decimals,
            qty_decimals: f.qty_decimals,
        };
        let entry = st.platform.entry(&f.canonical);
        if let Some(e) = &entry {
            if let Some((pd, qd)) = storage_precision(e) {
                if e.source == f.source.tag() && (pd, qd) != (f.price_decimals, f.qty_decimals) {
                    eprintln!(
                        "[instruments] {}: точность каталога {pd}/{qd}, точность хранения {}/{} закреплена при первом появлении — цены приводятся к ней (ТД-002)",
                        f.canonical, f.price_decimals, f.qty_decimals
                    );
                }
            }
        }
        *f = instrument_from(&row, entry.as_ref(), legacy.get(&f.id));
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    Unchanged,
    Applied { active: usize, skipped: Vec<String> },
}

pub async fn apply_list(st: &AppState, list: &PlatformList, source: &'static str) -> Applied {
    let _guard = st.platform.apply.lock().await;
    {
        let cat = st.catalog.read().unwrap_or_else(|e| e.into_inner());
        if cat.source == source && cat.version == Some(list.version) {
            let resolved = resolve_platform(&cat, list);
            let retry = list
                .instruments
                .iter()
                .any(|e| cat.id_of(&e.symbol).is_none() && st.platform.entry(&e.symbol).is_some());
            if resolved == cat.platform && !retry {
                return Applied::Unchanged;
            }
        }
    }
    let mut skipped = Vec::new();
    let mut refreshed = false;
    for e in &list.instruments {
        let known = st
            .catalog
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .id_of(&e.symbol);
        if known.is_some() {
            continue;
        }
        let mut entry = st.platform.entry(&e.symbol);
        if entry.is_none() && !refreshed {
            refreshed = true;
            refresh_entries(st, false).await;
            entry = st.platform.entry(&e.symbol);
        }
        let Some(entry) = entry else {
            eprintln!(
                "[instruments] {}: нет в каталоге модуля — пропущен, остальные применяются",
                e.symbol
            );
            skipped.push(e.symbol.clone());
            continue;
        };
        let Some((pd, qd)) = storage_precision(&entry) else {
            eprintln!(
                "[instruments] {}: точность из каталога не выводится (tick {}, step {}, contract {:?}) — пропущен",
                e.symbol, entry.tick_size, entry.qty_step, entry.contract_size
            );
            skipped.push(e.symbol.clone());
            continue;
        };
        let new = persistence::NewInstrument {
            symbol: entry.symbol.clone(),
            source: entry.source.clone(),
            provider_symbol: entry.provider_symbol.clone(),
            price_decimals: pd,
            qty_decimals: qd,
        };
        match st.store.register_instrument(&new).await {
            Ok(row) => {
                let f = instrument_from(&row, Some(&entry), None);
                println!(
                    "[instruments] {}: зарегистрирован, id {}, точность хранения {}/{}",
                    row.symbol, row.id, row.price_decimals, row.qty_decimals
                );
                if f.source == Source::V2 {
                    let status = entry
                        .status
                        .as_deref()
                        .and_then(Status::parse)
                        .unwrap_or(Status::Closed);
                    st.markets
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .entry(f.id)
                        .or_insert_with(|| MarketState::new(status));
                }
                st.catalog
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .by_id
                    .insert(row.id, f);
            }
            Err(err) => {
                eprintln!(
                    "[instruments] {}: регистрация не удалась ({err}) — пропущен",
                    new.symbol
                );
                skipped.push(new.symbol.clone());
            }
        }
    }
    let active = {
        let mut cat = st.catalog.write().unwrap_or_else(|e| e.into_inner());
        cat.platform = resolve_platform(&cat, list);
        cat.source = source;
        cat.version = Some(list.version);
        cat.updated_at = list.updated_at.clone();
        cat.platform.len()
    };
    if source == "cms" {
        if let Some(path) = st.platform.cache_path.as_deref() {
            let catalog: Vec<CatalogEntry> = list
                .instruments
                .iter()
                .filter_map(|e| st.platform.entry(&e.symbol))
                .collect();
            let file = CacheFile {
                saved_at: crate::now_ms(),
                list: list.clone(),
                catalog,
            };
            if let Err(e) = save_cache(path, &file) {
                eprintln!(
                    "[instruments] список не сохранён в {} ({e}) — при недоступной CMS после рестарта будет прежний",
                    path.display()
                );
            }
        }
    }
    println!(
        "[instruments] применён список платформы ({source}): version {}, активных {active}, пропущено {}",
        list.version,
        skipped.len()
    );
    recompute_subscriptions(st).await;
    Applied::Applied { active, skipped }
}

pub async fn poll_once(st: &AppState) -> Result<Applied, String> {
    let Some(cms) = st.platform.cms.clone() else {
        return Err("CMS не сконфигурирована".into());
    };
    let url = format!("{}/cms/platform/instruments", cms.base);
    let resp = st
        .platform
        .http
        .get(&url)
        .header("X-API-Key", &cms.api_key)
        .send()
        .await
        .map_err(|e| format!("сеть: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("JSON: {e}"))?;
    let list = parse_platform_list(&v)?;
    Ok(apply_list(st, &list, "cms").await)
}

pub async fn exposure_ids(st: &AppState) -> BTreeSet<u32> {
    let b = st.broker.lock().await;
    let mut out = BTreeSet::new();
    for u in b.users() {
        out.extend(b.positions(u).iter().map(|p| p.instrument));
        out.extend(b.pendings(u).iter().map(|p| p.instrument));
    }
    out
}

pub async fn exposure_of(st: &AppState, id: u32) -> (usize, usize) {
    let b = st.broker.lock().await;
    let mut positions = 0;
    let mut pendings = 0;
    for u in b.users() {
        positions += b.positions(u).iter().filter(|p| p.instrument == id).count();
        pendings += b.pendings(u).iter().filter(|p| p.instrument == id).count();
    }
    (positions, pendings)
}

pub async fn recompute_subscriptions(st: &AppState) {
    let exposed = exposure_ids(st).await;
    let (v1, v2) = {
        let cat = st.catalog.read().unwrap_or_else(|e| e.into_inner());
        let mut v1 = BTreeSet::new();
        let mut v2 = BTreeSet::new();
        let ids = cat
            .platform
            .iter()
            .map(|(id, _)| *id)
            .chain(exposed.iter().copied());
        for id in ids {
            if let Some(f) = cat.get(id) {
                match f.source {
                    Source::V1 => v1.insert(f.binance.clone()),
                    Source::V2 => v2.insert(f.binance.clone()),
                };
            }
        }
        (v1, v2)
    };
    st.platform.subs_v1.send_if_modified(|cur| {
        if *cur == v1 {
            false
        } else {
            *cur = v1;
            true
        }
    });
    st.platform.subs_v2.send_if_modified(|cur| {
        if *cur == v2 {
            false
        } else {
            *cur = v2;
            true
        }
    });
}

pub fn spawn(st: AppState) {
    let poller = st.clone();
    tokio::spawn(async move {
        refresh_entries(&poller, true).await;
        if poller.platform.cms.is_none() {
            return;
        }
        let mut tick = tokio::time::interval(Duration::from_secs(POLL_SECS));
        loop {
            tick.tick().await;
            if let Err(e) = poll_once(&poller).await {
                eprintln!(
                    "[instruments] список платформы из CMS не получен ({e}) — работаем по текущему"
                );
            }
        }
    });
    let refresher = st.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(CATALOG_REFRESH_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            refresh_entries(&refresher, true).await;
        }
    });
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            tick.tick().await;
            recompute_subscriptions(&st).await;
        }
    });
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/internal/hooks/platform-instruments", post(webhook))
        .route("/internal/instruments/:symbol/exposure", get(exposure))
}

fn fail(code: StatusCode, error: &str) -> Response {
    (code, Json(serde_json::json!({ "error": error }))).into_response()
}

async fn webhook(State(st): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let (Some(_), Some(secret)) = (&st.platform.cms, st.platform.webhook_secret.clone()) else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "webhook_disabled");
    };
    let header = headers.get("x-signature").and_then(|h| h.to_str().ok());
    if !verify_signature(&body, header, &secret) {
        return fail(StatusCode::UNAUTHORIZED, "bad_signature");
    }
    let bg = st.clone();
    tokio::spawn(async move {
        if let Err(e) = poll_once(&bg).await {
            eprintln!("[instruments] вебхук принят, но список не получен ({e})");
        }
    });
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "accepted": true })),
    )
        .into_response()
}

async fn exposure(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(symbol): Path<String>,
) -> Response {
    if let Err(denied) = money::authorize(&st, &headers) {
        return *denied;
    }
    let id = st.symbol_index().get(&normalize_symbol(&symbol)).copied();
    let (positions, pendings) = match id {
        Some(id) => exposure_of(&st, id).await,
        None => (0, 0),
    };
    let canonical = id
        .and_then(|id| st.catalog_get(id))
        .map(|f| f.canonical)
        .unwrap_or_else(|| symbol.trim().to_uppercase());
    Json(serde_json::json!({
        "symbol": canonical,
        "openPositions": positions,
        "pendingOrders": pendings,
    }))
    .into_response()
}
