pub mod money;
mod pg;

pub use pg::PgStore;

use async_trait::async_trait;

use broker::journal::AccountEvent;
use broker::AccountSnapshot;
use domain::account::{AccountKind, UserId};

#[derive(Debug, Clone)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "store: {}", self.0)
    }
}
impl std::error::Error for StoreError {}

#[derive(Debug, Default)]
pub struct LoadedState {
    pub accounts: Vec<(UserId, AccountSnapshot)>,
    pub journals: Vec<(UserId, Vec<AccountEvent>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentRow {
    pub id: u32,
    pub symbol: String,
    pub source: String,
    pub provider_symbol: String,
    pub price_decimals: u8,
    pub qty_decimals: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInstrument {
    pub symbol: String,
    pub source: String,
    pub provider_symbol: String,
    pub price_decimals: u8,
    pub qty_decimals: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRecord {
    pub event_id: String,
    pub routing_key: String,
    pub payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxClaim {
    pub id: i64,
    pub event_id: String,
    pub routing_key: String,
    pub payload: String,
    pub attempts: i32,
}

#[allow(clippy::double_must_use)]
#[async_trait]
pub trait Persistence: Send + Sync {
    async fn init(&self) -> Result<(), StoreError>;

    async fn load_all(&self) -> Result<LoadedState, StoreError>;

    async fn save_account(
        &self,
        user: UserId,
        snap: &AccountSnapshot,
        events: &[AccountEvent],
        outgoing: &[OutboxRecord],
    ) -> Result<(), StoreError>;

    async fn claim_outbox(
        &self,
        _limit: i64,
        _lease_ms: i64,
    ) -> Result<Vec<OutboxClaim>, StoreError> {
        Err(StoreError("хранилище не ведёт исходящую очередь".into()))
    }

    async fn mark_published(&self, _id: i64) -> Result<(), StoreError> {
        Err(StoreError("хранилище не ведёт исходящую очередь".into()))
    }

    async fn mark_failed(
        &self,
        _id: i64,
        _attempts: i32,
        _retry_in_ms: i64,
        _error: &str,
    ) -> Result<(), StoreError> {
        Err(StoreError("хранилище не ведёт исходящую очередь".into()))
    }

    async fn outbox_backlog(&self) -> Result<i64, StoreError> {
        Err(StoreError("хранилище не ведёт исходящую очередь".into()))
    }

    async fn identity_of(&self, _user: UserId) -> Result<Option<(String, String)>, StoreError> {
        Ok(None)
    }

    async fn resolve_identity(&self, _tenant: &str, _sub: &str) -> Result<UserId, StoreError> {
        Err(StoreError("identity mapping requires a database".into()))
    }

    async fn resolve_account(
        &self,
        tenant: &str,
        sub: &str,
        kind: AccountKind,
    ) -> Result<UserId, StoreError> {
        match kind {
            AccountKind::Demo => self.resolve_identity(tenant, sub).await,
            AccountKind::Real => Err(StoreError("real accounts require a database".into())),
        }
    }

    async fn find_account(
        &self,
        _tenant: &str,
        _sub: &str,
        _kind: AccountKind,
    ) -> Result<Option<UserId>, StoreError> {
        Ok(None)
    }

    async fn account_kind_of(&self, _user: UserId) -> Result<AccountKind, StoreError> {
        Ok(AccountKind::Demo)
    }

    async fn load_site_min_stops(&self) -> Result<Vec<(String, i64)>, StoreError> {
        Ok(Vec::new())
    }

    async fn save_site_min_stop(&self, _tenant: &str, _bps: i64) -> Result<(), StoreError> {
        Err(StoreError("хранилище не держит настройки площадок".into()))
    }

    async fn load_instruments(&self) -> Result<Vec<InstrumentRow>, StoreError> {
        Ok(Vec::new())
    }

    async fn pin_instruments(&self, _rows: &[InstrumentRow]) -> Result<(), StoreError> {
        Ok(())
    }

    async fn register_instrument(&self, _new: &NewInstrument) -> Result<InstrumentRow, StoreError> {
        Err(StoreError("хранилище не держит реестр инструментов".into()))
    }

    fn durable(&self) -> bool {
        false
    }

    fn kind(&self) -> &'static str {
        "memory"
    }
}

#[derive(Default)]
pub struct MemoryStore {
    accounts: std::sync::Mutex<std::collections::HashMap<UserId, AccountSnapshot>>,
    journals: std::sync::Mutex<std::collections::HashMap<UserId, Vec<AccountEvent>>>,
    outbox: std::sync::Mutex<Vec<MemoryOutboxRow>>,
    next_outbox_id: std::sync::atomic::AtomicI64,
    site_min_stops: std::sync::Mutex<std::collections::BTreeMap<String, i64>>,
    instruments: std::sync::Mutex<std::collections::BTreeMap<u32, InstrumentRow>>,
}

const MEMORY_FIRST_FREE_ID: u32 = 31;

#[derive(Debug, Clone)]
struct MemoryOutboxRow {
    id: i64,
    event_id: String,
    routing_key: String,
    payload: String,
    attempts: i32,
    published: bool,
    next_attempt_at: i64,
    last_error: Option<String>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    pub fn outbox_dump(&self) -> Vec<(String, i32, bool, Option<String>)> {
        self.outbox
            .lock()
            .map(|q| {
                q.iter()
                    .map(|r| {
                        (
                            r.event_id.clone(),
                            r.attempts,
                            r.published,
                            r.last_error.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[async_trait]
impl Persistence for MemoryStore {
    async fn init(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn load_all(&self) -> Result<LoadedState, StoreError> {
        let accounts = self
            .accounts
            .lock()
            .map_err(|_| StoreError("память счетов испорчена".into()))?;
        let journals = self
            .journals
            .lock()
            .map_err(|_| StoreError("память журналов испорчена".into()))?;
        let mut acc: Vec<(UserId, AccountSnapshot)> =
            accounts.iter().map(|(u, s)| (*u, s.clone())).collect();
        acc.sort_by_key(|(u, _)| u.0);
        let mut jr: Vec<(UserId, Vec<AccountEvent>)> =
            journals.iter().map(|(u, e)| (*u, e.clone())).collect();
        jr.sort_by_key(|(u, _)| u.0);
        Ok(LoadedState {
            accounts: acc,
            journals: jr,
        })
    }

    async fn save_account(
        &self,
        user: UserId,
        snap: &AccountSnapshot,
        events: &[AccountEvent],
        outgoing: &[OutboxRecord],
    ) -> Result<(), StoreError> {
        let mut accounts = self
            .accounts
            .lock()
            .map_err(|_| StoreError("память счетов испорчена".into()))?;
        let mut journals = self
            .journals
            .lock()
            .map_err(|_| StoreError("память журналов испорчена".into()))?;
        let lane = journals.entry(user).or_default();
        for e in events {
            let expected = lane.last().map(|x: &AccountEvent| x.seq()).unwrap_or(0) + 1;
            if e.seq() != expected {
                return Err(StoreError(format!(
                    "журнал счёта {}: ожидалась запись №{expected}, пришла №{}",
                    user.0,
                    e.seq()
                )));
            }
            lane.push(e.clone());
        }
        {
            use std::sync::atomic::Ordering;
            let mut q = self
                .outbox
                .lock()
                .map_err(|_| StoreError("память исходящей очереди испорчена".into()))?;
            for rec in outgoing {
                if q.iter().any(|r| r.event_id == rec.event_id) {
                    continue;
                }
                let id = self.next_outbox_id.fetch_add(1, Ordering::Relaxed) + 1;
                q.push(MemoryOutboxRow {
                    id,
                    event_id: rec.event_id.clone(),
                    routing_key: rec.routing_key.clone(),
                    payload: rec.payload.clone(),
                    attempts: 0,
                    published: false,
                    next_attempt_at: 0,
                    last_error: None,
                });
            }
        }
        accounts.insert(user, snap.clone());
        Ok(())
    }

    async fn claim_outbox(
        &self,
        limit: i64,
        lease_ms: i64,
    ) -> Result<Vec<OutboxClaim>, StoreError> {
        let now = Self::now_ms();
        let mut q = self
            .outbox
            .lock()
            .map_err(|_| StoreError("память исходящей очереди испорчена".into()))?;
        let mut out = Vec::new();
        for r in q.iter_mut() {
            if out.len() as i64 >= limit {
                break;
            }
            if r.published || r.next_attempt_at > now {
                continue;
            }
            r.next_attempt_at = now + lease_ms;
            out.push(OutboxClaim {
                id: r.id,
                event_id: r.event_id.clone(),
                routing_key: r.routing_key.clone(),
                payload: r.payload.clone(),
                attempts: r.attempts,
            });
        }
        Ok(out)
    }

    async fn mark_published(&self, id: i64) -> Result<(), StoreError> {
        let mut q = self
            .outbox
            .lock()
            .map_err(|_| StoreError("память исходящей очереди испорчена".into()))?;
        match q.iter_mut().find(|r| r.id == id) {
            Some(r) => {
                r.published = true;
                Ok(())
            }
            None => Err(StoreError(format!(
                "исходящая очередь: строки №{id} нет, пометить отправленной нечего"
            ))),
        }
    }

    async fn mark_failed(
        &self,
        id: i64,
        attempts: i32,
        retry_in_ms: i64,
        error: &str,
    ) -> Result<(), StoreError> {
        let now = Self::now_ms();
        let mut q = self
            .outbox
            .lock()
            .map_err(|_| StoreError("память исходящей очереди испорчена".into()))?;
        match q.iter_mut().find(|r| r.id == id) {
            Some(r) => {
                r.attempts = attempts;
                r.next_attempt_at = now + retry_in_ms;
                r.last_error = Some(error.chars().take(500).collect());
                Ok(())
            }
            None => Err(StoreError(format!(
                "исходящая очередь: строки №{id} нет, записать отказ некуда"
            ))),
        }
    }

    async fn outbox_backlog(&self) -> Result<i64, StoreError> {
        let q = self
            .outbox
            .lock()
            .map_err(|_| StoreError("память исходящей очереди испорчена".into()))?;
        Ok(q.iter().filter(|r| !r.published).count() as i64)
    }

    async fn load_site_min_stops(&self) -> Result<Vec<(String, i64)>, StoreError> {
        let m = self
            .site_min_stops
            .lock()
            .map_err(|_| StoreError("память настроек испорчена".into()))?;
        Ok(m.iter().map(|(t, b)| (t.clone(), *b)).collect())
    }

    async fn save_site_min_stop(&self, tenant: &str, bps: i64) -> Result<(), StoreError> {
        self.site_min_stops
            .lock()
            .map_err(|_| StoreError("память настроек испорчена".into()))?
            .insert(tenant.to_string(), bps);
        Ok(())
    }

    async fn load_instruments(&self) -> Result<Vec<InstrumentRow>, StoreError> {
        let m = self
            .instruments
            .lock()
            .map_err(|_| StoreError("память реестра инструментов испорчена".into()))?;
        Ok(m.values().cloned().collect())
    }

    async fn pin_instruments(&self, rows: &[InstrumentRow]) -> Result<(), StoreError> {
        let mut m = self
            .instruments
            .lock()
            .map_err(|_| StoreError("память реестра инструментов испорчена".into()))?;
        for r in rows {
            if m.values().any(|x| x.symbol == r.symbol && x.id != r.id) {
                continue;
            }
            m.entry(r.id).or_insert_with(|| r.clone());
        }
        Ok(())
    }

    async fn register_instrument(&self, new: &NewInstrument) -> Result<InstrumentRow, StoreError> {
        let mut m = self
            .instruments
            .lock()
            .map_err(|_| StoreError("память реестра инструментов испорчена".into()))?;
        if let Some(found) = m.values().find(|x| x.symbol == new.symbol) {
            return Ok(found.clone());
        }
        let id = m
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            .max(MEMORY_FIRST_FREE_ID - 1)
            + 1;
        let row = InstrumentRow {
            id,
            symbol: new.symbol.clone(),
            source: new.source.clone(),
            provider_symbol: new.provider_symbol.clone(),
            price_decimals: new.price_decimals,
            qty_decimals: new.qty_decimals,
        };
        m.insert(id, row.clone());
        Ok(row)
    }

    async fn identity_of(&self, _user: UserId) -> Result<Option<(String, String)>, StoreError> {
        Ok(None)
    }
}
