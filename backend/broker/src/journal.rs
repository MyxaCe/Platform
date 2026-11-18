use std::fmt::Write as _;

use domain::account::{
    AccountKind, AccountOrigin, AccountProfile, CategoryAtOpen, ClientCategory, Currency, UserId,
};

use crate::{
    margin_for, merged_entry, realized_on_close, AccountSnapshot, Cents, ClosedDeal, DealFees,
    Fees, PendingOrder, PosSide, Position, PositionFees, PositionModel,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operator {
    who: String,
    reason: String,
    incident: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadOperator(pub &'static str);

impl std::fmt::Display for BadOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl Operator {
    pub fn new(who: &str, reason: &str, incident: &str) -> Result<Self, BadOperator> {
        if who.trim().is_empty() {
            return Err(BadOperator("ручная проводка без исполнителя"));
        }
        if reason.trim().is_empty() {
            return Err(BadOperator("ручная проводка без основания"));
        }
        if incident.trim().is_empty() {
            return Err(BadOperator("ручная проводка без ссылки на инцидент"));
        }
        Ok(Self {
            who: who.trim().to_string(),
            reason: reason.trim().to_string(),
            incident: incident.trim().to_string(),
        })
    }

    pub fn who(&self) -> &str {
        &self.who
    }
    pub fn reason(&self) -> &str {
        &self.reason
    }
    pub fn incident(&self) -> &str {
        &self.incident
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    System,
    Client,
    Operator(Operator),
}

impl Origin {
    pub fn tag(&self) -> &'static str {
        match self {
            Origin::System => "system",
            Origin::Client => "client",
            Origin::Operator(_) => "operator",
        }
    }

    pub fn operator(&self) -> Option<&Operator> {
        match self {
            Origin::Operator(o) => Some(o),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseCause {
    Command,
    StopLoss,
    TakeProfit,
    Netting,
    StopOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    AccountOpened {
        balance: Cents,
        profile: AccountProfile,
    },
    ModelAdopted {
        model: PositionModel,
    },
    Deposited {
        amount: Cents,
    },
    Withdrawn {
        amount: Cents,
    },
    PositionOpened {
        id: u64,
        instrument: u32,
        side: PosSide,
        qty: i64,
        entry: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
        lev: i128,
    },
    PositionIncreased {
        id: u64,
        add_qty: i64,
        add_entry: i64,
        sl: Option<i64>,
        tp: Option<i64>,
    },
    PositionReduced {
        id: u64,
        qty: i64,
        exit: i64,
    },
    PositionClosed {
        id: u64,
        exit: i64,
        cause: CloseCause,
        level: Option<i64>,
        across_gap: bool,
    },
    PendingPlaced {
        id: u64,
        instrument: u32,
        side: PosSide,
        qty: i64,
        price: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
    },
    PendingCancelled {
        id: u64,
    },
    PendingTriggered {
        id: u64,
        price: i64,
        exec: Option<i64>,
        across_gap: bool,
    },
    DemoAccountReset {
        adjustment: Cents,
    },
    NegativeBalanceCompensated {
        amount: Cents,
    },
    HistoryExcludedFromReporting {
        through_seq: u64,
    },
    TradingFreezeSet {
        frozen: bool,
    },
    LeverageSet {
        lev: i128,
    },
    PendingRejected {
        id: u64,
        exec: Option<i64>,
        reason: RejectReason,
    },
    FeesSet {
        markup_bps: i64,
        commission_bps: i64,
    },
    MarkupApplied {
        trade: u64,
        mkt: i64,
        exec: i64,
        amount: Cents,
    },
    CommissionCharged {
        amount: Cents,
        trade: u64,
        waived: Cents,
    },
    PositionLevelsChanged {
        id: u64,
        sl: Option<i64>,
        tp: Option<i64>,
    },
    PendingLevelsChanged {
        id: u64,
        sl: Option<i64>,
        tp: Option<i64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    AccountFrozen,
}

impl RejectReason {
    pub fn tag(self) -> &'static str {
        match self {
            RejectReason::AccountFrozen => "account_frozen",
        }
    }
}

impl EventKind {
    pub fn changes_state(&self) -> bool {
        match self {
            EventKind::AccountOpened { .. }
            | EventKind::ModelAdopted { .. }
            | EventKind::Deposited { .. }
            | EventKind::Withdrawn { .. }
            | EventKind::PositionOpened { .. }
            | EventKind::PositionIncreased { .. }
            | EventKind::PositionReduced { .. }
            | EventKind::PositionClosed { .. }
            | EventKind::PendingPlaced { .. }
            | EventKind::PendingCancelled { .. }
            | EventKind::PendingTriggered { .. }
            | EventKind::NegativeBalanceCompensated { .. }
            | EventKind::DemoAccountReset { .. }
            | EventKind::TradingFreezeSet { .. }
            | EventKind::LeverageSet { .. }
            | EventKind::PendingRejected { .. }
            | EventKind::FeesSet { .. }
            | EventKind::MarkupApplied { .. }
            | EventKind::CommissionCharged { .. }
            | EventKind::PositionLevelsChanged { .. }
            | EventKind::PendingLevelsChanged { .. } => true,
            EventKind::HistoryExcludedFromReporting { .. } => false,
        }
    }
}

fn opt(v: Option<i64>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => "-".to_string(),
    }
}

fn flag(v: bool) -> &'static str {
    if v {
        "1"
    } else {
        "0"
    }
}

fn side_tag(s: PosSide) -> &'static str {
    match s {
        PosSide::Long => "long",
        PosSide::Short => "short",
    }
}

fn fields(rest: &str) -> Result<Vec<(&str, &str)>, BadRecord> {
    rest.split_whitespace()
        .map(|kv| {
            kv.split_once('=')
                .ok_or(BadRecord("поле записи без знака равенства"))
        })
        .collect()
}

fn pick<'a>(fs: &[(&'a str, &'a str)], key: &str) -> Result<&'a str, BadRecord> {
    fs.iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .ok_or(BadRecord("в записи нет обязательного поля"))
}

fn num<T: std::str::FromStr>(fs: &[(&str, &str)], key: &str) -> Result<T, BadRecord> {
    pick(fs, key)?
        .parse()
        .map_err(|_| BadRecord("поле записи не число"))
}

fn num_opt(fs: &[(&str, &str)], key: &str) -> Result<Option<i64>, BadRecord> {
    match pick(fs, key)? {
        "-" => Ok(None),
        v => v
            .parse()
            .map(Some)
            .map_err(|_| BadRecord("поле записи не число")),
    }
}

fn num_opt_added_later(fs: &[(&str, &str)], key: &str) -> Result<Option<i64>, BadRecord> {
    match fs.iter().find(|(k, _)| *k == key) {
        None => Ok(None),
        Some((_, "-")) => Ok(None),
        Some((_, v)) => v
            .parse()
            .map(Some)
            .map_err(|_| BadRecord("поле записи не число")),
    }
}

fn flag_added_later(fs: &[(&str, &str)], key: &str) -> Result<bool, BadRecord> {
    match fs.iter().find(|(k, _)| *k == key) {
        None => Ok(false),
        Some((_, "0")) => Ok(false),
        Some((_, "1")) => Ok(true),
        Some(_) => Err(BadRecord("поле-признак не 0 и не 1")),
    }
}

fn profile_added_later(fs: &[(&str, &str)]) -> Result<AccountProfile, BadRecord> {
    let owner = match fs.iter().find(|(k, _)| *k == "owner") {
        None => UserId(0),
        Some((_, v)) => UserId(v.parse().map_err(|_| BadRecord("owner не число"))?),
    };
    let pick_enum = |key: &str| fs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    let kind = match pick_enum("kind") {
        None => AccountKind::Demo,
        Some(v) => AccountKind::parse(v).ok_or(BadRecord("неизвестный род счёта"))?,
    };
    let origin = match pick_enum("origin") {
        None => AccountOrigin::Registration,
        Some(v) => AccountOrigin::parse(v).ok_or(BadRecord("неизвестное происхождение счёта"))?,
    };
    let category_at_open = match pick_enum("category_at_open") {
        None => CategoryAtOpen(ClientCategory::Retail),
        Some(v) => CategoryAtOpen(
            ClientCategory::parse(v).ok_or(BadRecord("неизвестная категория клиента"))?,
        ),
    };
    let currency = match pick_enum("currency") {
        None => Currency::USD,
        Some(v) => Currency::parse(v).ok_or(BadRecord("не код валюты"))?,
    };
    Ok(AccountProfile {
        owner,
        kind,
        origin,
        category_at_open,
        currency,
    })
}

fn lev_added_later(fs: &[(&str, &str)]) -> Result<i128, BadRecord> {
    match fs.iter().find(|(k, _)| *k == "lev") {
        None => Ok(1),
        Some((_, v)) => match v.parse::<i128>() {
            Ok(l) if l > 0 => Ok(l),
            _ => Err(BadRecord("плечо записи не положительное число")),
        },
    }
}

fn side_of(fs: &[(&str, &str)], key: &str) -> Result<PosSide, BadRecord> {
    match pick(fs, key)? {
        "long" => Ok(PosSide::Long),
        "short" => Ok(PosSide::Short),
        _ => Err(BadRecord("неизвестная сторона позиции")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadRecord(pub &'static str);

impl std::fmt::Display for BadRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl EventKind {
    pub fn encode(&self) -> String {
        match self {
            EventKind::AccountOpened { balance, profile } => format!(
                "account_opened balance={balance} owner={} kind={} origin={} category_at_open={} currency={}",
                profile.owner.0,
                profile.kind.tag(),
                profile.origin.tag(),
                profile.category_at_open.0.tag(),
                profile.currency.code()
            ),
            EventKind::ModelAdopted { model } => {
                let m = match model {
                    PositionModel::Hedging => "hedging",
                    PositionModel::Netting => "netting",
                };
                format!("model_adopted model={m}")
            }
            EventKind::Deposited { amount } => format!("deposited amount={amount}"),
            EventKind::Withdrawn { amount } => format!("withdrawn amount={amount}"),
            EventKind::PositionOpened { id, instrument, side, qty, entry, pd, qd, sl, tp, lev } => format!(
                "position_opened id={id} instrument={instrument} side={} qty={qty} entry={entry} pd={pd} qd={qd} sl={} tp={} lev={lev}",
                side_tag(*side),
                opt(*sl),
                opt(*tp)
            ),
            EventKind::PositionIncreased { id, add_qty, add_entry, sl, tp } => format!(
                "position_increased id={id} add_qty={add_qty} add_entry={add_entry} sl={} tp={}",
                opt(*sl),
                opt(*tp)
            ),
            EventKind::PositionReduced { id, qty, exit } => format!("position_reduced id={id} qty={qty} exit={exit}"),
            EventKind::PositionClosed { id, exit, cause, level, across_gap } => {
                let c = match cause {
                    CloseCause::Command => "command",
                    CloseCause::StopLoss => "stop_loss",
                    CloseCause::TakeProfit => "take_profit",
                    CloseCause::Netting => "netting",
                    CloseCause::StopOut => "stop_out",
                };
                format!("position_closed id={id} exit={exit} cause={c} level={} gap={}", opt(*level), flag(*across_gap))
            }
            EventKind::PendingPlaced { id, instrument, side, qty, price, pd, qd, sl, tp } => format!(
                "pending_placed id={id} instrument={instrument} side={} qty={qty} price={price} pd={pd} qd={qd} sl={} tp={}",
                side_tag(*side),
                opt(*sl),
                opt(*tp)
            ),
            EventKind::PendingCancelled { id } => format!("pending_cancelled id={id}"),
            EventKind::PendingTriggered { id, price, exec, across_gap } => {
                format!("pending_triggered id={id} price={price} exec={} gap={}", opt(*exec), flag(*across_gap))
            }
            EventKind::HistoryExcludedFromReporting { through_seq } => {
                format!("history_excluded_from_reporting through_seq={through_seq}")
            }
            EventKind::NegativeBalanceCompensated { amount } => {
                format!("negative_balance_compensated amount={amount}")
            }
            EventKind::DemoAccountReset { adjustment } => {
                format!("demo_account_reset adjustment={adjustment}")
            }
            EventKind::TradingFreezeSet { frozen } => {
                format!("trading_freeze_set frozen={}", flag(*frozen))
            }
            EventKind::LeverageSet { lev } => format!("leverage_set lev={lev}"),
            EventKind::PendingRejected { id, exec, reason } => {
                format!("pending_rejected id={id} exec={} reason={}", opt(*exec), reason.tag())
            }
            EventKind::FeesSet { markup_bps, commission_bps } => {
                format!("fees_set markup_bps={markup_bps} commission_bps={commission_bps}")
            }
            EventKind::MarkupApplied { trade, mkt, exec, amount } => {
                format!("markup_applied trade={trade} mkt={mkt} exec={exec} amount={amount}")
            }
            EventKind::CommissionCharged { amount, trade, waived } => {
                if *waived == 0 {
                    format!("commission_charged amount={amount} trade={trade}")
                } else {
                    format!("commission_charged amount={amount} trade={trade} waived={waived}")
                }
            }
            EventKind::PositionLevelsChanged { id, sl, tp } => {
                format!("position_levels_changed id={id} sl={} tp={}", opt(*sl), opt(*tp))
            }
            EventKind::PendingLevelsChanged { id, sl, tp } => {
                format!("pending_levels_changed id={id} sl={} tp={}", opt(*sl), opt(*tp))
            }
        }
    }

    pub fn decode(s: &str) -> Result<EventKind, BadRecord> {
        let (tag, rest) = s.split_once(' ').unwrap_or((s, ""));
        let f = fields(rest)?;
        Ok(match tag {
            "account_opened" => {
                let balance = num(&f, "balance")?;
                EventKind::AccountOpened {
                    balance,
                    profile: profile_added_later(&f)?,
                }
            }
            "model_adopted" => EventKind::ModelAdopted {
                model: match pick(&f, "model")? {
                    "hedging" => PositionModel::Hedging,
                    "netting" => PositionModel::Netting,
                    _ => return Err(BadRecord("неизвестная модель позиций")),
                },
            },
            "deposited" => EventKind::Deposited {
                amount: num(&f, "amount")?,
            },
            "withdrawn" => EventKind::Withdrawn {
                amount: num(&f, "amount")?,
            },
            "position_opened" => EventKind::PositionOpened {
                id: num(&f, "id")?,
                instrument: num(&f, "instrument")?,
                side: side_of(&f, "side")?,
                qty: num(&f, "qty")?,
                entry: num(&f, "entry")?,
                pd: num(&f, "pd")?,
                qd: num(&f, "qd")?,
                sl: num_opt(&f, "sl")?,
                tp: num_opt(&f, "tp")?,
                lev: lev_added_later(&f)?,
            },
            "position_increased" => EventKind::PositionIncreased {
                id: num(&f, "id")?,
                add_qty: num(&f, "add_qty")?,
                add_entry: num(&f, "add_entry")?,
                sl: num_opt(&f, "sl")?,
                tp: num_opt(&f, "tp")?,
            },
            "position_reduced" => EventKind::PositionReduced {
                id: num(&f, "id")?,
                qty: num(&f, "qty")?,
                exit: num(&f, "exit")?,
            },
            "position_closed" => EventKind::PositionClosed {
                id: num(&f, "id")?,
                exit: num(&f, "exit")?,
                cause: match pick(&f, "cause")? {
                    "command" => CloseCause::Command,
                    "stop_loss" => CloseCause::StopLoss,
                    "take_profit" => CloseCause::TakeProfit,
                    "netting" => CloseCause::Netting,
                    "stop_out" => CloseCause::StopOut,
                    _ => return Err(BadRecord("неизвестная причина закрытия")),
                },
                level: num_opt_added_later(&f, "level")?,
                across_gap: flag_added_later(&f, "gap")?,
            },
            "pending_placed" => EventKind::PendingPlaced {
                id: num(&f, "id")?,
                instrument: num(&f, "instrument")?,
                side: side_of(&f, "side")?,
                qty: num(&f, "qty")?,
                price: num(&f, "price")?,
                pd: num(&f, "pd")?,
                qd: num(&f, "qd")?,
                sl: num_opt(&f, "sl")?,
                tp: num_opt(&f, "tp")?,
            },
            "pending_cancelled" => EventKind::PendingCancelled { id: num(&f, "id")? },
            "pending_triggered" => EventKind::PendingTriggered {
                id: num(&f, "id")?,
                price: num(&f, "price")?,
                exec: num_opt_added_later(&f, "exec")?,
                across_gap: flag_added_later(&f, "gap")?,
            },
            "sandbox_declared" | "history_excluded_from_reporting" => {
                EventKind::HistoryExcludedFromReporting {
                    through_seq: num(&f, "through_seq")?,
                }
            }
            "negative_balance_compensated" => EventKind::NegativeBalanceCompensated {
                amount: num(&f, "amount")?,
            },
            "demo_account_reset" => EventKind::DemoAccountReset {
                adjustment: num(&f, "adjustment")?,
            },
            "trading_freeze_set" => EventKind::TradingFreezeSet {
                frozen: match pick(&f, "frozen")? {
                    "0" => false,
                    "1" => true,
                    _ => return Err(BadRecord("поле-признак не 0 и не 1")),
                },
            },
            "leverage_set" => EventKind::LeverageSet {
                lev: match num::<i128>(&f, "lev")? {
                    l if (1..=crate::MAX_LEVERAGE).contains(&l) => l,
                    _ => return Err(BadRecord("плечо записи вне 1..=1000")),
                },
            },
            "pending_rejected" => EventKind::PendingRejected {
                id: num(&f, "id")?,
                exec: num_opt(&f, "exec")?,
                reason: match pick(&f, "reason")? {
                    "account_frozen" => RejectReason::AccountFrozen,
                    _ => return Err(BadRecord("неизвестная причина отказа ордера")),
                },
            },
            "fees_set" => {
                let bps = |key: &str| -> Result<i64, BadRecord> {
                    match num::<i64>(&f, key)? {
                        v if (0..=crate::MAX_FEE_BPS).contains(&v) => Ok(v),
                        _ => Err(BadRecord("ставка тарифа записи вне 0..=MAX_FEE_BPS")),
                    }
                };
                EventKind::FeesSet {
                    markup_bps: bps("markup_bps")?,
                    commission_bps: bps("commission_bps")?,
                }
            }
            "markup_applied" => EventKind::MarkupApplied {
                trade: num(&f, "trade")?,
                mkt: num(&f, "mkt")?,
                exec: num(&f, "exec")?,
                amount: match num::<Cents>(&f, "amount")? {
                    a if a >= 0 => a,
                    _ => return Err(BadRecord("отрицательная наценка")),
                },
            },
            "commission_charged" => {
                let amount = num::<Cents>(&f, "amount")?;
                let waived = match f.iter().find(|(k, _)| *k == "waived") {
                    None => 0,
                    Some((_, v)) => v
                        .parse::<Cents>()
                        .map_err(|_| BadRecord("поле записи не число"))?,
                };
                if amount < 0 || waived < 0 {
                    return Err(BadRecord("отрицательная комиссия"));
                }
                EventKind::CommissionCharged {
                    amount,
                    trade: num(&f, "trade")?,
                    waived,
                }
            }
            "position_levels_changed" | "pending_levels_changed" => {
                let id = num(&f, "id")?;
                let sl = num_opt(&f, "sl")?;
                let tp = num_opt(&f, "tp")?;
                if sl.is_some_and(|v| v <= 0) || tp.is_some_and(|v| v <= 0) {
                    return Err(BadRecord("уровень SL/TP записи не положительный"));
                }
                if tag == "position_levels_changed" {
                    EventKind::PositionLevelsChanged { id, sl, tp }
                } else {
                    EventKind::PendingLevelsChanged { id, sl, tp }
                }
            }
            _ => return Err(BadRecord("неизвестный вид события")),
        })
    }
}

fn fingerprint(bytes: &[u8]) -> u128 {
    const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013b;
    let mut h = OFFSET;
    for b in bytes {
        h ^= *b as u128;
        h = h.wrapping_mul(PRIME);
    }
    h
}

fn origin_canon(o: &Origin) -> String {
    match o {
        Origin::System => "system".to_string(),
        Origin::Client => "client".to_string(),
        Origin::Operator(op) => {
            let mut s = String::from("operator");
            for (k, v) in [
                ("who", op.who()),
                ("reason", op.reason()),
                ("incident", op.incident()),
            ] {
                let _ = write!(s, "|{k}:{}:{v}", v.len());
            }
            s
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountEvent {
    seq: u64,
    at_ms: i64,
    origin: Origin,
    kind: EventKind,
    canon: String,
    prev: u128,
    hash: u128,
}

impl AccountEvent {
    pub fn seq(&self) -> u64 {
        self.seq
    }
    pub fn at_ms(&self) -> i64 {
        self.at_ms
    }
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
    pub fn kind(&self) -> &EventKind {
        &self.kind
    }
    pub fn prev(&self) -> u128 {
        self.prev
    }
    pub fn hash(&self) -> u128 {
        self.hash
    }

    fn digest(&self) -> u128 {
        let text = format!(
            "{}|{}|{}|{}|{:032x}",
            self.seq,
            self.at_ms,
            origin_canon(&self.origin),
            self.canon,
            self.prev
        );
        fingerprint(text.as_bytes())
    }

    pub fn canon(&self) -> &str {
        &self.canon
    }

    #[cfg(test)]
    pub fn digest_for_tests(&self) -> u128 {
        self.digest()
    }

    pub fn from_storage(
        seq: u64,
        at_ms: i64,
        origin: Origin,
        canon: &str,
        prev: u128,
        hash: u128,
    ) -> Result<Self, BadRecord> {
        Ok(Self {
            seq,
            at_ms,
            origin,
            kind: EventKind::decode(canon)?,
            canon: canon.to_string(),
            prev,
            hash,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayState {
    pub opened: bool,
    pub balance: Cents,
    pub next_id: u64,
    pub positions: Vec<Position>,
    pub pendings: Vec<PendingOrder>,
    pub closed: Vec<ClosedDeal>,
    pub model: PositionModel,
    pub orphans: Vec<u64>,
    pub profile: AccountProfile,
    pub excluded_from_reporting_through: Option<u64>,
    pub trading_frozen: bool,
    pub leverage: Option<i128>,
    pub fees: Fees,
    last_mkt: Option<(u64, i64)>,
}

impl ReplayState {
    fn new(start_balance: Cents) -> Self {
        Self {
            opened: false,
            balance: start_balance,
            next_id: 0,
            positions: Vec::new(),
            pendings: Vec::new(),
            closed: Vec::new(),
            model: PositionModel::default(),
            orphans: Vec::new(),
            profile: AccountProfile::default(),
            excluded_from_reporting_through: None,
            trading_frozen: false,
            leverage: None,
            fees: Fees::default(),
            last_mkt: None,
        }
    }

    pub fn used_margin(&self) -> Cents {
        self.positions.iter().map(|p| p.margin).sum()
    }

    fn pos_mut(&mut self, id: u64) -> Option<&mut Position> {
        self.positions.iter_mut().find(|p| p.id == id)
    }

    fn apply(&mut self, kind: &EventKind) {
        let minor = self.profile.currency.minor_per_unit();
        if !kind.changes_state() {
            if let EventKind::HistoryExcludedFromReporting { through_seq } = kind {
                self.excluded_from_reporting_through = Some(*through_seq);
            }
            return;
        }
        match kind {
            EventKind::AccountOpened { balance, profile } => {
                self.opened = true;
                self.balance = *balance;
                self.profile = *profile;
            }
            EventKind::ModelAdopted { model } => self.model = *model,
            EventKind::Deposited { amount } => self.balance += *amount,
            EventKind::NegativeBalanceCompensated { amount } => self.balance += *amount,
            EventKind::DemoAccountReset { adjustment } => self.balance += *adjustment,
            EventKind::TradingFreezeSet { frozen } => self.trading_frozen = *frozen,
            EventKind::LeverageSet { lev } => self.leverage = Some(*lev),
            EventKind::FeesSet {
                markup_bps,
                commission_bps,
            } => {
                self.fees = Fees {
                    markup_bps: *markup_bps,
                    commission_bps: *commission_bps,
                }
            }
            EventKind::MarkupApplied {
                trade, mkt, amount, ..
            } => {
                let Some(p) = self.pos_mut(*trade) else {
                    self.orphans.push(*trade);
                    return;
                };
                p.fees.markup += *amount;
                self.last_mkt = Some((*trade, *mkt));
            }
            EventKind::CommissionCharged { amount, trade, .. } => {
                self.balance -= *amount;
                let Some(p) = self.pos_mut(*trade) else {
                    self.orphans.push(*trade);
                    return;
                };
                p.fees.commission += *amount;
            }
            EventKind::Withdrawn { amount } => self.balance -= *amount,
            EventKind::PositionOpened {
                id,
                instrument,
                side,
                qty,
                entry,
                pd,
                qd,
                sl,
                tp,
                lev: rec_lev,
            } => {
                self.next_id = self.next_id.max(*id);
                self.positions.push(Position {
                    leverage: *rec_lev,
                    id: *id,
                    instrument: *instrument,
                    side: *side,
                    qty: *qty,
                    entry: *entry,
                    pd: *pd,
                    qd: *qd,
                    margin: margin_for(*entry, *qty, *pd, *qd, *rec_lev, minor),
                    sl: *sl,
                    tp: *tp,
                    fees: PositionFees::snapshot(self.fees),
                });
                self.last_mkt = None;
            }
            EventKind::PositionIncreased {
                id,
                add_qty,
                add_entry,
                sl,
                tp,
            } => {
                self.last_mkt = None;
                let Some(p) = self.pos_mut(*id) else {
                    self.orphans.push(*id);
                    return;
                };
                let new_qty = p.qty + add_qty;
                let new_entry = merged_entry(p.qty, p.entry, *add_qty, *add_entry);
                p.qty = new_qty;
                p.entry = new_entry;
                p.margin = margin_for(new_entry, new_qty, p.pd, p.qd, p.leverage, minor);
                if sl.is_some() {
                    p.sl = *sl;
                }
                if tp.is_some() {
                    p.tp = *tp;
                }
            }
            EventKind::PositionReduced { id, qty, exit } => {
                let market_exit = match self.last_mkt.take() {
                    Some((pid, m)) if pid == *id => Some(m),
                    _ => Some(*exit),
                };
                let Some(p) = self.pos_mut(*id) else {
                    self.orphans.push(*id);
                    return;
                };
                let pnl = realized_on_close(p.side, p.entry, *exit, *qty, p.pd, p.qd, minor);
                let fees = DealFees {
                    commission: p.fees.commission,
                    markup: p.fees.markup,
                    market_exit,
                };
                p.fees.commission = 0;
                p.fees.markup = 0;
                let deal = ClosedDeal {
                    instrument: p.instrument,
                    side: p.side,
                    qty: *qty,
                    entry: p.entry,
                    exit: *exit,
                    pnl,
                    pd: p.pd,
                    qd: p.qd,
                    level: None,
                    across_gap: Some(false),
                    fees,
                };
                p.qty -= qty;
                let left = p.qty;
                p.margin = margin_for(p.entry, left, p.pd, p.qd, p.leverage, minor);
                self.balance += pnl;
                self.closed.push(deal);
            }
            EventKind::PositionClosed {
                id,
                exit,
                cause: _,
                level,
                across_gap,
            } => {
                let Some(i) = self.positions.iter().position(|p| p.id == *id) else {
                    self.orphans.push(*id);
                    return;
                };
                let p = self.positions.remove(i);
                let pnl = realized_on_close(p.side, p.entry, *exit, p.qty, p.pd, p.qd, minor);
                let market_exit = match self.last_mkt.take() {
                    Some((pid, m)) if pid == *id => Some(m),
                    _ => Some(*exit),
                };
                self.balance += pnl;
                self.closed.push(ClosedDeal {
                    instrument: p.instrument,
                    side: p.side,
                    qty: p.qty,
                    entry: p.entry,
                    exit: *exit,
                    pnl,
                    pd: p.pd,
                    qd: p.qd,
                    level: *level,
                    across_gap: Some(*across_gap),
                    fees: DealFees {
                        commission: p.fees.commission,
                        markup: p.fees.markup,
                        market_exit,
                    },
                });
            }
            EventKind::PendingPlaced {
                id,
                instrument,
                side,
                qty,
                price,
                pd,
                qd,
                sl,
                tp,
            } => {
                self.next_id = self.next_id.max(*id);
                self.pendings.push(PendingOrder {
                    id: *id,
                    instrument: *instrument,
                    side: *side,
                    qty: *qty,
                    price: *price,
                    pd: *pd,
                    qd: *qd,
                    sl: *sl,
                    tp: *tp,
                });
            }
            EventKind::PositionLevelsChanged { id, sl, tp } => {
                let Some(p) = self.pos_mut(*id) else {
                    self.orphans.push(*id);
                    return;
                };
                p.sl = *sl;
                p.tp = *tp;
            }
            EventKind::PendingLevelsChanged { id, sl, tp } => {
                let Some(o) = self.pendings.iter_mut().find(|o| o.id == *id) else {
                    self.orphans.push(*id);
                    return;
                };
                o.sl = *sl;
                o.tp = *tp;
            }
            EventKind::PendingCancelled { id }
            | EventKind::PendingTriggered { id, .. }
            | EventKind::PendingRejected { id, .. } => {
                let before = self.pendings.len();
                self.pendings.retain(|p| p.id != *id);
                if self.pendings.len() == before {
                    self.orphans.push(*id);
                }
            }
            EventKind::HistoryExcludedFromReporting { .. } => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalFault {
    Sequence { expected: u64, got: u64 },
    Chain { seq: u64 },
    Tampered { seq: u64 },
}

impl std::fmt::Display for JournalFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JournalFault::Sequence { expected, got } => {
                write!(f, "журнал: ожидалась запись №{expected}, пришла №{got} — запись вставлена или потеряна")
            }
            JournalFault::Chain { seq } => write!(f, "журнал: запись №{seq} не продолжает цепочку"),
            JournalFault::Tampered { seq } => {
                write!(f, "журнал: запись №{seq} изменена после записи")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Journal {
    events: Vec<AccountEvent>,
    state: ReplayState,
    start_balance: Cents,
}

impl Journal {
    pub fn new(start_balance: Cents) -> Self {
        Self {
            events: Vec::new(),
            state: ReplayState::new(start_balance),
            start_balance,
        }
    }

    pub fn load(start_balance: Cents, events: Vec<AccountEvent>) -> Result<Self, JournalFault> {
        let mut j = Journal::new(start_balance);
        j.events = events;
        j.verify()?;
        j.state = j.replay();
        Ok(j)
    }

    pub fn events(&self) -> &[AccountEvent] {
        &self.events
    }
    pub fn len(&self) -> usize {
        self.events.len()
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
    pub fn state(&self) -> &ReplayState {
        &self.state
    }

    pub fn seal(&self, at_ms: i64, origin: &Origin, kinds: &[EventKind]) -> Vec<AccountEvent> {
        let mut out = Vec::with_capacity(kinds.len());
        let last_seq = self.events.last().map(|e| e.seq).unwrap_or(0);
        let mut prev = self.events.last().map(|e| e.hash).unwrap_or(0);
        for (i, k) in kinds.iter().enumerate() {
            let seq = last_seq + i as u64 + 1;
            let mut e = AccountEvent {
                seq,
                at_ms,
                origin: origin.clone(),
                kind: k.clone(),
                canon: k.encode(),
                prev,
                hash: 0,
            };
            e.hash = e.digest();
            prev = e.hash;
            out.push(e);
        }
        out
    }

    pub fn preview(&self, events: &[AccountEvent]) -> ReplayState {
        let mut s = self.state.clone();
        for e in events {
            s.apply(&e.kind);
        }
        s
    }

    pub fn commit(&mut self, e: AccountEvent) -> Result<(), JournalFault> {
        let expected_seq = self.events.last().map(|x| x.seq).unwrap_or(0) + 1;
        if e.seq != expected_seq {
            return Err(JournalFault::Sequence {
                expected: expected_seq,
                got: e.seq,
            });
        }
        let expected_prev = self.events.last().map(|x| x.hash).unwrap_or(0);
        if e.prev != expected_prev {
            return Err(JournalFault::Chain { seq: e.seq });
        }
        if e.hash != e.digest() {
            return Err(JournalFault::Tampered { seq: e.seq });
        }
        self.state.apply(&e.kind);
        self.events.push(e);
        Ok(())
    }

    pub fn verify(&self) -> Result<(), JournalFault> {
        let mut prev = 0u128;
        for (i, e) in self.events.iter().enumerate() {
            let expected_seq = i as u64 + 1;
            if e.seq != expected_seq {
                return Err(JournalFault::Sequence {
                    expected: expected_seq,
                    got: e.seq,
                });
            }
            if e.prev != prev {
                return Err(JournalFault::Chain { seq: e.seq });
            }
            if e.hash != e.digest() {
                return Err(JournalFault::Tampered { seq: e.seq });
            }
            prev = e.hash;
        }
        Ok(())
    }

    pub fn replay(&self) -> ReplayState {
        let mut s = ReplayState::new(self.start_balance);
        for e in &self.events {
            s.apply(&e.kind);
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence(pub String);

impl std::fmt::Display for Divergence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalStatus {
    Reconciled,
    NoEntries,
    Faulted(JournalFault),
    Diverged(Vec<Divergence>),
    NoProvenance(Vec<Divergence>),
}

impl JournalStatus {
    pub fn ok(&self) -> bool {
        matches!(self, JournalStatus::Reconciled)
    }
    pub fn tag(&self) -> &'static str {
        match self {
            JournalStatus::Reconciled => "reconciled",
            JournalStatus::NoEntries => "no_entries",
            JournalStatus::Faulted(_) => "faulted",
            JournalStatus::Diverged(_) => "diverged",
            JournalStatus::NoProvenance(_) => "no_provenance",
        }
    }
}

impl std::fmt::Display for JournalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JournalStatus::Reconciled => f.write_str("журнал сходится со слепком"),
            JournalStatus::NoEntries => f.write_str("журнала нет ни одной записи, а счёт не пуст"),
            JournalStatus::Faulted(x) => write!(f, "{x}"),
            JournalStatus::Diverged(ds) => {
                let all: Vec<String> = ds.iter().map(|d| d.0.clone()).collect();
                write!(f, "расхождение журнала со слепком: {}", all.join("; "))
            }
            JournalStatus::NoProvenance(ds) => {
                let all: Vec<String> = ds.iter().map(|d| d.0.clone()).collect();
                write!(f, "деньги без происхождения: {}", all.join("; "))
            }
        }
    }
}

pub fn provenance(state: &ReplayState) -> Option<Divergence> {
    (!state.opened).then(|| {
        Divergence(
            "происхождение: в журнале нет записи о заведении счёта — первые деньги взялись ниоткуда"
                .into(),
        )
    })
}

pub fn reconcile(state: &ReplayState, snap: &AccountSnapshot) -> Vec<Divergence> {
    let mut out = Vec::new();
    let mut d = |msg: String| out.push(Divergence(msg));

    if let Some(v) = provenance(state) {
        d(v.0);
    }

    if state.balance != snap.balance {
        d(format!(
            "баланс: по журналу {}, в слепке {}",
            state.balance, snap.balance
        ));
    }
    let used_journal = state.used_margin();
    let used_snap: Cents = snap.positions.iter().map(|p| p.margin).sum();
    if used_journal != used_snap {
        d(format!(
            "занятая маржа: по журналу {used_journal}, в слепке {used_snap}"
        ));
    }
    if state.next_id != snap.next_id {
        d(format!(
            "счётчик id: по журналу {}, в слепке {}",
            state.next_id, snap.next_id
        ));
    }
    if state.model != snap.model {
        d(format!(
            "модель позиций: по журналу {:?}, в слепке {:?}",
            state.model, snap.model
        ));
    }
    if state.leverage != snap.leverage {
        d(format!(
            "плечо счёта: по журналу {:?}, в слепке {:?}",
            state.leverage, snap.leverage
        ));
    }
    if state.fees != snap.fees {
        d(format!(
            "тариф счёта: по журналу {:?}, в слепке {:?}",
            state.fees, snap.fees
        ));
    }
    for id in &state.orphans {
        d(format!(
            "событие журнала ссылается на неизвестную позицию или ордер {id}"
        ));
    }

    let mut jp = state.positions.clone();
    let mut sp = snap.positions.clone();
    jp.sort_by_key(|p| p.id);
    sp.sort_by_key(|p| p.id);
    let jids: Vec<u64> = jp.iter().map(|p| p.id).collect();
    let sids: Vec<u64> = sp.iter().map(|p| p.id).collect();
    if jids != sids {
        d(format!(
            "открытые позиции: по журналу {jids:?}, в слепке {sids:?}"
        ));
    } else {
        for (a, b) in jp.iter().zip(sp.iter()) {
            if a.instrument != b.instrument
                || a.side != b.side
                || a.qty != b.qty
                || a.entry != b.entry
            {
                d(format!(
                    "позиция {}: по журналу {:?} {} @ {} (инструмент {}), в слепке {:?} {} @ {} (инструмент {})",
                    a.id, a.side, a.qty, a.entry, a.instrument, b.side, b.qty, b.entry, b.instrument
                ));
            }
            if a.margin != b.margin {
                d(format!(
                    "позиция {}: маржа по журналу {}, в слепке {}",
                    a.id, a.margin, b.margin
                ));
            }
            if a.fees != b.fees {
                d(format!(
                    "позиция {}: тариф/доход площадки по журналу {:?}, в слепке {:?}",
                    a.id, a.fees, b.fees
                ));
            }
            if a.sl != b.sl || a.tp != b.tp {
                d(format!(
                    "позиция {}: SL/TP по журналу {:?}/{:?}, в слепке {:?}/{:?}",
                    a.id, a.sl, a.tp, b.sl, b.tp
                ));
            }
        }
    }

    let mut jo: Vec<u64> = state.pendings.iter().map(|p| p.id).collect();
    let mut so: Vec<u64> = snap.pendings.iter().map(|p| p.id).collect();
    jo.sort_unstable();
    so.sort_unstable();
    if jo != so {
        d(format!(
            "отложенные ордера: по журналу {jo:?}, в слепке {so:?}"
        ));
    } else {
        let mut jp = state.pendings.clone();
        let mut sp = snap.pendings.clone();
        jp.sort_by_key(|o| o.id);
        sp.sort_by_key(|o| o.id);
        for (a, b) in jp.iter().zip(sp.iter()) {
            if a.sl != b.sl || a.tp != b.tp {
                d(format!(
                    "ордер {}: SL/TP по журналу {:?}/{:?}, в слепке {:?}/{:?}",
                    a.id, a.sl, a.tp, b.sl, b.tp
                ));
            }
        }
    }

    if state.closed.len() != snap.closed.len() {
        d(format!(
            "история сделок: по журналу {} записей, в слепке {}",
            state.closed.len(),
            snap.closed.len()
        ));
    } else {
        for (i, (a, b)) in state.closed.iter().zip(snap.closed.iter()).enumerate() {
            if a.qty != b.qty
                || a.entry != b.entry
                || a.exit != b.exit
                || a.pnl != b.pnl
                || a.side != b.side
            {
                d(format!(
                    "сделка {i}: по журналу {} @ {}→{} P&L {}, в слепке {} @ {}→{} P&L {}",
                    a.qty, a.entry, a.exit, a.pnl, b.qty, b.entry, b.exit, b.pnl
                ));
            }
            if a.fees.commission != b.fees.commission || a.fees.markup != b.fees.markup {
                d(format!(
                    "сделка {i}: комиссия/наценка по журналу {}/{}, в слепке {}/{}",
                    a.fees.commission, a.fees.markup, b.fees.commission, b.fees.markup
                ));
            }
        }
    }

    out
}

pub fn audit(journal: &Journal, snap: &AccountSnapshot) -> JournalStatus {
    if let Err(f) = journal.verify() {
        return JournalStatus::Faulted(f);
    }
    let replayed = journal.replay();
    if journal.is_empty() {
        let fresh = replayed.balance == snap.balance
            && snap.positions.is_empty()
            && snap.pendings.is_empty()
            && snap.closed.is_empty()
            && snap.next_id == 0
            && snap.leverage.is_none()
            && snap.fees.is_zero();
        return if fresh {
            JournalStatus::Reconciled
        } else {
            JournalStatus::NoEntries
        };
    }
    let ds = reconcile(&replayed, snap);
    if ds.is_empty() {
        return JournalStatus::Reconciled;
    }
    if provenance(&replayed).is_some() && ds.len() == 1 {
        return JournalStatus::NoProvenance(ds);
    }
    JournalStatus::Diverged(ds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops() -> Vec<EventKind> {
        vec![
            EventKind::AccountOpened {
                balance: 10_000_000,
                profile: Default::default(),
            },
            EventKind::ModelAdopted {
                model: PositionModel::Netting,
            },
            EventKind::Deposited { amount: 50_000 },
            EventKind::Withdrawn { amount: 1 },
            EventKind::PositionOpened {
                id: 1,
                instrument: 7,
                side: PosSide::Long,
                qty: 100,
                entry: 5000,
                pd: 2,
                qd: 3,
                sl: Some(4900),
                tp: None,
                lev: 1,
            },
            EventKind::PositionIncreased {
                id: 1,
                add_qty: 300,
                add_entry: 5400,
                sl: None,
                tp: Some(5500),
            },
            EventKind::PositionReduced {
                id: 1,
                qty: 150,
                exit: 5600,
            },
            EventKind::PositionClosed {
                id: 1,
                exit: 5100,
                cause: CloseCause::StopLoss,
                level: Some(5050),
                across_gap: true,
            },
            EventKind::PendingPlaced {
                id: 2,
                instrument: 3,
                side: PosSide::Short,
                qty: 10,
                price: 4000,
                pd: 2,
                qd: 3,
                sl: None,
                tp: None,
            },
            EventKind::PendingCancelled { id: 2 },
            EventKind::PendingTriggered {
                id: 2,
                price: 4000,
                exec: Some(3990),
                across_gap: false,
            },
            EventKind::HistoryExcludedFromReporting { through_seq: 11 },
            EventKind::TradingFreezeSet { frozen: true },
            EventKind::TradingFreezeSet { frozen: false },
            EventKind::LeverageSet { lev: 50 },
            EventKind::PendingRejected {
                id: 2,
                exec: Some(3990),
                reason: RejectReason::AccountFrozen,
            },
            EventKind::PendingRejected {
                id: 2,
                exec: None,
                reason: RejectReason::AccountFrozen,
            },
        ]
    }

    #[test]
    fn every_event_survives_the_round_trip() {
        for k in ops() {
            let text = k.encode();
            assert_eq!(
                EventKind::decode(&text),
                Ok(k.clone()),
                "не пережило round-trip: {text}"
            );
        }
    }
    #[test]
    fn historical_records_from_production_still_verify() {
        const LIVE: [(&str, u128); 3] = [
            (
                "account_opened balance=1000000",
                0xbf80fb04e1805c85c4822be3f2d413f6,
            ),
            (
                "position_opened id=1 instrument=2 side=long qty=1000 entry=239883 pd=2 qd=4 sl=- tp=-",
                0xc97803856631f06eff21501ca384bdb1,
            ),
            (
                "position_closed id=1 exit=240053 cause=command level=- gap=0",
                0xc669479698096e277e5cbcaaebee2367,
            ),
        ];
        for (text, hash) in LIVE {
            let e = AccountEvent::from_storage(1, 1_700_000_000_000, Origin::Client, text, 0, hash)
                .unwrap_or_else(|err| panic!("{text}: {err}"));
            assert_eq!(e.canon(), text, "канон приезжает как есть");
            Journal::load(10_000_000, vec![e])
                .unwrap_or_else(|f| panic!("{text}: запись с контура обязана сверяться, а не {f}"));
        }
    }

    #[test]
    fn a_pre_profile_account_opened_reads_as_a_legacy_demo_account() {
        let e = AccountEvent::from_storage(
            1,
            1_700_000_000_000,
            Origin::Client,
            "account_opened balance=1000000",
            0,
            0xbf80fb04e1805c85c4822be3f2d413f6,
        )
        .expect("разбор");
        match e.kind() {
            EventKind::AccountOpened { balance, profile } => {
                assert_eq!(*balance, 1_000_000);
                assert_eq!(
                    profile.kind,
                    AccountKind::Demo,
                    "другого рода тогда не было"
                );
                assert_eq!(profile.origin, AccountOrigin::Registration);
                assert_eq!(profile.currency, Currency::USD);
                assert_eq!(
                    profile.category_at_open,
                    CategoryAtOpen(ClientCategory::Retail),
                    "и все клиенты считались розничными"
                );
            }
            other => panic!("не та запись: {other:?}"),
        }
        Journal::load(10_000_000, vec![e]).expect("и сверяется прежним отпечатком");
    }

    #[test]
    fn a_record_written_before_the_shape_changed_still_verifies() {
        const OLD: &str = "position_closed id=1 exit=240053 cause=command";
        const AT: i64 = 1_700_000_000_000;
        const SIGNED_BACK_THEN: u128 = 0xcb7a4435a2bf0a5f9c3f3b48cca93ae0;

        let stored = AccountEvent::from_storage(1, AT, Origin::Client, OLD, 0, SIGNED_BACK_THEN)
            .expect("разбор обязан быть терпимым к отсутствию полей");
        assert_eq!(stored.canon(), OLD, "канон обязан сохраниться как есть");

        let j = Journal::load(10_000_000, vec![stored])
            .expect("журнал со старой записью обязан подниматься, а не уходить в карантин");
        assert_eq!(j.events().len(), 1);
        assert_eq!(
            j.events()[0].canon(),
            OLD,
            "и остаться в своём виде: переписывать прошлое мы не вправе"
        );
    }

    #[test]
    fn the_new_shape_fingerprint_does_not_fit_the_old_record() {
        const OLD: &str = "position_closed id=1 exit=240053 cause=command";
        const SIGNED_AS_IF_NEW: u128 = 0xc669479698096e277e5cbcaaebee2367;

        let stored = AccountEvent::from_storage(
            1,
            1_700_000_000_000,
            Origin::Client,
            OLD,
            0,
            SIGNED_AS_IF_NEW,
        )
        .expect("разбор");
        let fault = Journal::load(10_000_000, vec![stored])
            .expect_err("чужой отпечаток обязан ловиться — иначе первый тест ничего не значит");
        assert_eq!(fault, JournalFault::Tampered { seq: 1 });
    }

    #[test]
    fn the_fold_takes_the_leverage_from_the_record() {
        const AT_LEV_1: Cents = 23_988;
        const AT_LEV_100: Cents = 239;

        for (lev, want) in [(1i128, AT_LEV_1), (100, AT_LEV_100)] {
            let rec = format!(
                "position_opened id=1 instrument=2 side=long qty=1000 entry=239883                  pd=2 qd=4 sl=- tp=- lev={lev}"
            )
            .replace("                 ", "");
            let kind = EventKind::decode(&rec).expect("запись читается");
            let mut st = ReplayState::new(1_000_000);
            st.profile = AccountProfile::legacy_demo(UserId(1));
            st.apply(&kind);
            assert_eq!(
                st.positions.first().expect("позиция восстановилась").margin,
                want,
                "запись с плечом {lev} обязана свернуться в маржу {want}"
            );
        }
    }

    #[test]
    fn increase_and_reduce_follow_the_position_leverage() {
        const OPEN_AT_100: &str = "position_opened id=1 instrument=2 side=long qty=1000 entry=239883 pd=2 qd=4 sl=- tp=- lev=100";
        const MARGIN_1K: Cents = 239;
        const MARGIN_2K: Cents = 479;

        let mut st = ReplayState::new(1_000_000);
        st.profile = AccountProfile::legacy_demo(UserId(1));
        st.apply(&EventKind::decode(OPEN_AT_100).expect("запись читается"));
        assert_eq!(st.positions[0].margin, MARGIN_1K, "открытие при плече 100");

        st.apply(&EventKind::PositionIncreased {
            id: 1,
            add_qty: 1000,
            add_entry: 239_883,
            sl: None,
            tp: None,
        });
        assert_eq!(
            st.positions[0].margin, MARGIN_2K,
            "доливка считается плечом позиции (100), а не единицей"
        );

        st.apply(&EventKind::PositionReduced {
            id: 1,
            qty: 1000,
            exit: 239_883,
        });
        assert_eq!(
            st.positions[0].margin, MARGIN_1K,
            "частичное закрытие тоже считается плечом позиции"
        );
    }

    #[test]
    fn a_pre_leverage_opening_replays_at_leverage_one() {
        const OLD: &str =
            "position_opened id=1 instrument=2 side=long qty=1000 entry=239883 pd=2 qd=4 sl=- tp=-";
        const MARGIN_AT_LEV_1: Cents = 23_988;

        let kind = EventKind::decode(OLD).expect("старая запись обязана читаться");
        let mut st = ReplayState::new(1_000_000);
        st.profile = AccountProfile::legacy_demo(UserId(1));
        st.apply(&kind);

        let p = st
            .positions
            .first()
            .expect("позиция обязана восстановиться");
        assert_eq!(
            p.margin, MARGIN_AT_LEV_1,
            "маржа старой записи обязана считаться по плечу 1 — другого у неё не было"
        );
        assert_eq!(
            p.leverage, 1,
            "у записи без поля плечо обязано быть единицей, а не нулём и не чем-то ещё"
        );
    }

    #[test]
    fn a_record_under_the_old_name_still_verifies() {
        const OLD: &str = "sandbox_declared through_seq=2";
        const SIGNED_BACK_THEN: u128 = 0x2a4a8f7a89b547aa4d0f6db4fc006da5;

        let e = AccountEvent::from_storage(3, 1_700_000_000_000, Origin::Client, OLD, 0, 0)
            .expect("старое имя обязано разбираться");
        assert_eq!(
            e.kind(),
            &EventKind::HistoryExcludedFromReporting { through_seq: 2 },
            "читается НОВЫМ видом"
        );
        assert_eq!(e.canon(), OLD, "а подписана прежним текстом");
        assert_eq!(
            e.digest_for_tests(),
            SIGNED_BACK_THEN,
            "и отпечаток тот же, что был посчитан тогда"
        );
    }

    #[test]
    fn exactly_one_kind_is_skipped_by_the_fold() {
        let all = ops();
        let skipped: Vec<&EventKind> = all.iter().filter(|k| !k.changes_state()).collect();
        assert_eq!(
            skipped.len(),
            1,
            "пропускаться обязан ровно один вид записи, а не список: {skipped:?}"
        );
        assert!(
            matches!(skipped[0], EventKind::HistoryExcludedFromReporting { .. }),
            "и это объявление песочницы, а не что-то ещё: {:?}",
            skipped[0]
        );
    }

    #[test]
    fn the_sandbox_declaration_moves_nothing() {
        let mut s = ReplayState::new(1_000_000);
        s.apply(&EventKind::AccountOpened {
            balance: 1_000_000,
            profile: Default::default(),
        });
        s.apply(&EventKind::Deposited { amount: 7 });
        let before = s.clone();

        s.apply(&EventKind::HistoryExcludedFromReporting { through_seq: 2 });

        assert_eq!(s.balance, before.balance);
        assert_eq!(s.positions, before.positions);
        assert_eq!(s.pendings, before.pendings);
        assert_eq!(s.closed, before.closed);
        assert_eq!(s.next_id, before.next_id);
        assert_eq!(s.model, before.model);
        assert_eq!(s.opened, before.opened);
        assert_eq!(s.orphans, before.orphans);
        assert_eq!(
            s.excluded_from_reporting_through,
            Some(2),
            "изменилось ровно заявление"
        );
    }

    #[test]
    fn close_causes_are_distinguishable() {
        let causes = [
            CloseCause::Command,
            CloseCause::StopLoss,
            CloseCause::TakeProfit,
            CloseCause::Netting,
        ];
        let mut texts: Vec<String> = causes
            .iter()
            .map(|c| {
                EventKind::PositionClosed {
                    id: 1,
                    exit: 1,
                    cause: *c,
                    level: None,
                    across_gap: false,
                }
                .encode()
            })
            .collect();
        texts.sort();
        texts.dedup();
        assert_eq!(
            texts.len(),
            causes.len(),
            "причины закрытия обязаны различаться в записи"
        );
        for c in causes {
            let k = EventKind::PositionClosed {
                id: 1,
                exit: 1,
                cause: c,
                level: None,
                across_gap: false,
            };
            assert_eq!(EventKind::decode(&k.encode()), Ok(k));
        }
    }

    #[test]
    fn records_written_before_the_new_fields_still_read() {
        let old_close = "position_closed id=7 exit=5100 cause=stop_loss";
        assert_eq!(
            EventKind::decode(old_close),
            Ok(EventKind::PositionClosed {
                id: 7,
                exit: 5100,
                cause: CloseCause::StopLoss,
                level: None,
                across_gap: false,
            }),
            "старая запись закрытия перестала читаться — это стёртая история, а не строгость"
        );

        let old_fill = "pending_triggered id=9 price=4000";
        assert_eq!(
            EventKind::decode(old_fill),
            Ok(EventKind::PendingTriggered {
                id: 9,
                price: 4000,
                exec: None,
                across_gap: false
            })
        );
    }

    #[test]
    fn present_but_unreadable_new_field_is_still_corruption() {
        assert!(
            EventKind::decode("position_closed id=7 exit=5100 cause=stop_loss level=кот").is_err()
        );
        assert!(
            EventKind::decode("position_closed id=7 exit=5100 cause=stop_loss gap=может_быть")
                .is_err(),
            "признак разрыва принимает только 0 и 1: «может быть» в журнале не бывает"
        );
    }

    #[test]
    fn slippage_is_visible_in_the_record() {
        let quiet = EventKind::PositionClosed {
            id: 1,
            exit: 5000,
            cause: CloseCause::StopLoss,
            level: Some(5000),
            across_gap: false,
        };
        let slipped = EventKind::PositionClosed {
            id: 1,
            exit: 4200,
            cause: CloseCause::StopLoss,
            level: Some(5000),
            across_gap: true,
        };
        assert_ne!(quiet.encode(), slipped.encode());
        assert_eq!(EventKind::decode(&slipped.encode()), Ok(slipped));
    }

    #[test]
    fn unknown_record_is_refused_not_guessed() {
        assert!(EventKind::decode("teleported amount=1").is_err());
        assert!(
            EventKind::decode("deposited").is_err(),
            "нет обязательного поля — отказ"
        );
        assert!(EventKind::decode("deposited amount=много").is_err());
    }

    #[test]
    fn manual_entry_without_who_reason_or_incident_is_refused() {
        assert!(
            Operator::new("", "сбой PSP", "INC-42").is_err(),
            "без исполнителя"
        );
        assert!(
            Operator::new("ops@x", "   ", "INC-42").is_err(),
            "без основания"
        );
        assert!(
            Operator::new("ops@x", "сбой PSP", "").is_err(),
            "без инцидента"
        );
        assert!(Operator::new("ops@x", "сбой PSP", "INC-42").is_ok());
    }

    #[test]
    fn manual_posting_is_recorded_and_moves_the_replayed_balance() {
        let mut j = Journal::new(10_000_000);
        let op = Origin::Operator(
            Operator::new("ops@platform", "платёж прошёл у провайдера", "INC-42").unwrap(),
        );
        for e in j.seal(
            1_700_000_000_000,
            &Origin::Client,
            &[EventKind::AccountOpened {
                balance: 10_000_000,
                profile: Default::default(),
            }],
        ) {
            j.commit(e).unwrap();
        }
        for e in j.seal(
            1_700_000_001_000,
            &op,
            &[EventKind::Deposited { amount: 250_000 }],
        ) {
            j.commit(e).unwrap();
        }

        let last = j.events().last().unwrap();
        let who = last
            .origin()
            .operator()
            .expect("проводка обязана остаться ручной");
        assert_eq!(who.who(), "ops@platform");
        assert_eq!(who.reason(), "платёж прошёл у провайдера");
        assert_eq!(who.incident(), "INC-42");
        assert_eq!(
            last.at_ms(),
            1_700_000_001_000,
            "когда — часть записи, а не догадка по порядку"
        );
        assert_eq!(
            j.replay().balance,
            10_250_000,
            "остаток выводится из проводки, а не правится"
        );
    }

    #[test]
    fn origin_is_part_of_the_fingerprint() {
        let j = Journal::new(0);
        let k = [EventKind::Deposited { amount: 1 }];
        let a = j.seal(1, &Origin::System, &k);
        let b = j.seal(1, &Origin::Client, &k);
        assert_ne!(
            a[0].hash(),
            b[0].hash(),
            "источник обязан влиять на отпечаток"
        );
    }

    fn filled() -> Journal {
        let mut j = Journal::new(10_000_000);
        let kinds = [
            EventKind::AccountOpened {
                balance: 10_000_000,
                profile: Default::default(),
            },
            EventKind::Deposited { amount: 500 },
            EventKind::PositionOpened {
                id: 1,
                instrument: 1,
                side: PosSide::Long,
                qty: 1000,
                entry: 50_000,
                pd: 2,
                qd: 3,
                sl: None,
                tp: None,
                lev: 1,
            },
        ];
        for (i, k) in kinds.iter().enumerate() {
            for e in j.seal(1_000 + i as i64, &Origin::Client, std::slice::from_ref(k)) {
                j.commit(e).unwrap();
            }
        }
        j
    }

    #[test]
    fn edited_record_is_detected() {
        let j = filled();
        let mut rows: Vec<AccountEvent> = j.events().to_vec();
        let victim = &rows[1];
        rows[1] = AccountEvent::from_storage(
            victim.seq(),
            victim.at_ms(),
            victim.origin().clone(),
            &EventKind::Deposited { amount: 5_000_000 }.encode(),
            victim.prev(),
            victim.hash(),
        )
        .expect("строка разбирается");
        let faulted = Journal::load(10_000_000, rows).expect_err("правка обязана ловиться");
        assert_eq!(faulted, JournalFault::Tampered { seq: 2 });
    }

    #[test]
    fn edited_record_with_recomputed_fingerprint_breaks_the_chain() {
        let j = filled();
        let mut rows: Vec<AccountEvent> = j.events().to_vec();
        let victim = &rows[1];
        let forged = EventKind::Deposited { amount: 5_000_000 }.encode();
        let probe = AccountEvent::from_storage(
            victim.seq(),
            victim.at_ms(),
            victim.origin().clone(),
            &forged,
            victim.prev(),
            0,
        )
        .expect("строка разбирается");
        let h = probe.digest();
        rows[1] = AccountEvent::from_storage(
            probe.seq(),
            probe.at_ms(),
            probe.origin().clone(),
            &forged,
            probe.prev(),
            h,
        )
        .expect("строка разбирается");
        let faulted = Journal::load(10_000_000, rows).expect_err("подделка обязана ловиться");
        assert_eq!(
            faulted,
            JournalFault::Chain { seq: 3 },
            "следующая запись ссылается на прежний отпечаток"
        );
    }

    #[test]
    fn removed_record_is_detected() {
        let j = filled();
        let mut rows: Vec<AccountEvent> = j.events().to_vec();
        rows.remove(1);
        let faulted = Journal::load(10_000_000, rows).expect_err("пропажа обязана ловиться");
        assert_eq!(
            faulted,
            JournalFault::Sequence {
                expected: 2,
                got: 3
            }
        );
    }

    #[test]
    fn sealing_and_previewing_do_not_touch_the_journal() {
        let j = filled();
        let before_len = j.len();
        let before_balance = j.state().balance;
        let sealed = j.seal(
            9_999,
            &Origin::Client,
            &[EventKind::Deposited { amount: 777 }],
        );
        let preview = j.preview(&sealed);
        assert_eq!(j.len(), before_len, "лента не должна пополниться до записи");
        assert_eq!(
            j.state().balance,
            before_balance,
            "состояние журнала не должно измениться"
        );
        assert_eq!(
            preview.balance,
            before_balance + 777,
            "а предпросмотр обязан показать будущее"
        );
    }

    #[test]
    fn commit_refuses_a_record_that_does_not_continue_the_lane() {
        let mut j = filled();
        let mut two = j.seal(
            5_000,
            &Origin::Client,
            &[
                EventKind::Deposited { amount: 1 },
                EventKind::Deposited { amount: 2 },
            ],
        );
        let second = two.remove(1);
        let len_before = j.len();
        assert_eq!(
            j.commit(second),
            Err(JournalFault::Sequence {
                expected: 4,
                got: 5
            }),
            "дыра в ленте — отказ"
        );
        assert_eq!(
            j.len(),
            len_before,
            "отказавшая запись не должна попадать в ленту"
        );
    }

    #[test]
    fn maintained_state_matches_a_full_replay() {
        let j = filled();
        assert_eq!(j.replay(), *j.state());
    }

    fn snap_of(j: &Journal) -> AccountSnapshot {
        let s = j.replay();
        AccountSnapshot {
            profile: s.profile,
            balance: s.balance,
            next_id: s.next_id,
            positions: s.positions.clone(),
            pendings: s.pendings.clone(),
            closed: s.closed.clone(),
            model: s.model,
            leverage: s.leverage,
            fees: s.fees,
        }
    }

    #[test]
    fn replay_of_a_matching_snapshot_has_no_divergence() {
        let j = filled();
        assert_eq!(reconcile(&j.replay(), &snap_of(&j)), vec![]);
        assert_eq!(audit(&j, &snap_of(&j)), JournalStatus::Reconciled);
    }

    #[test]
    fn balance_drift_is_found() {
        let j = filled();
        let mut s = snap_of(&j);
        s.balance += 1;
        let ds = reconcile(&j.replay(), &s);
        assert!(
            ds.iter().any(|d| d.0.contains("баланс")),
            "расхождение остатка обязано ловиться: {ds:?}"
        );
    }

    #[test]
    fn missing_position_and_margin_drift_are_found() {
        let j = filled();
        let mut s = snap_of(&j);
        s.positions.clear();
        assert!(reconcile(&j.replay(), &s)
            .iter()
            .any(|d| d.0.contains("позиции")));

        let mut s2 = snap_of(&j);
        s2.positions[0].margin += 7;
        let ds = reconcile(&j.replay(), &s2);
        assert!(
            ds.iter().any(|d| d.0.contains("маржа")),
            "дрейф маржи обязан ловиться: {ds:?}"
        );
    }

    #[test]
    fn an_event_that_never_reached_the_journal_is_found() {
        let j = filled();
        let mut s = snap_of(&j);
        s.balance += 100_000;
        let status = audit(&j, &s);
        assert!(
            matches!(status, JournalStatus::Diverged(_)),
            "непрописанное пополнение обязано ловиться: {status}"
        );
    }

    #[test]
    fn empty_journal_over_a_live_account_says_no_entries() {
        let j = Journal::new(10_000_000);
        let live = AccountSnapshot {
            profile: Default::default(),
            balance: 10_000_500,
            next_id: 1,
            positions: vec![],
            pendings: vec![],
            closed: vec![],
            model: PositionModel::Hedging,
            leverage: None,
            fees: Default::default(),
        };
        assert_eq!(audit(&j, &live), JournalStatus::NoEntries);
        assert_eq!(audit(&j, &live).tag(), "no_entries");

        let fresh = AccountSnapshot {
            balance: 10_000_000,
            ..Default::default()
        };
        assert_eq!(audit(&j, &fresh), JournalStatus::Reconciled);
    }

    #[test]
    fn diagnostics_do_not_blur_into_one() {
        let mut tags = vec![
            JournalStatus::Reconciled.tag(),
            JournalStatus::NoEntries.tag(),
            JournalStatus::Faulted(JournalFault::Chain { seq: 1 }).tag(),
            JournalStatus::Diverged(vec![Divergence("x".into())]).tag(),
        ];
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(
            tags.len(),
            4,
            "диагнозы обязаны различаться кодом, а не только текстом"
        );
    }

    #[test]
    fn event_about_an_unknown_position_is_not_swallowed() {
        let mut j = Journal::new(0);
        for e in j.seal(
            1,
            &Origin::System,
            &[EventKind::PositionReduced {
                id: 42,
                qty: 1,
                exit: 1,
            }],
        ) {
            j.commit(e).unwrap();
        }
        assert_eq!(j.state().orphans, vec![42]);
        let ds = reconcile(&j.replay(), &AccountSnapshot::default());
        assert!(
            ds.iter().any(|d| d.0.contains("неизвестную позицию")),
            "{ds:?}"
        );
    }
}
