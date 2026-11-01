//! Порты плеча (TRM-53 шаг 3, Р-446): доставка входов к разрешению плеча.
//!
//! Разрешение снимка в одно число живёт на самом снимке
//! ([`AccountLeverageSnapshot::resolve`]); здесь — только **порты**, через
//! которые к нему приходят два входа, ни один из которых на 01.11 не имеет
//! согласованного канала:
//!
//! - **приём снимка site→terminal** — контракт ещё не согласован (витрина
//!   строит снимок, но не шлёт). Здесь трейт-приёмник [`LeverageSnapshots`] и
//!   его стаб в памяти [`InMemoryLeverageSnapshots`]; живую проводку изобретать
//!   нельзя — она вынесена штабу как контракт. [`InMemoryLeverageSnapshots::ingest`]
//!   — ровно та точка, в которую эта проводка встанет;
//! - **символ → класс инструмента** — открытый вопрос владельцу (CMS или
//!   терминал). Класс приходит через трейт [`InstrumentClasses`]; источник не
//!   зашит, стаб [`InMemoryInstrumentClasses`] хранит таблицу явно.
//!
//! Брокер остаётся in-memory и синхронным (принцип №6): трейты не несут I/O,
//! живой адаптер к витрине и к источнику классов живёт в `gateway`.

use std::collections::HashMap;

use domain::account::UserId;

pub use domain::leverage::{AccountLeverageSnapshot, InstrumentClass};

/// Источник снимков плеча — **ingestion-порт** (TRM-53).
///
/// `None` — снимка на счёт ещё нет: по плечу портфеля открывать нечего, и
/// вызывающий остаётся на честной основе (сегодня — плечо площадки), а не
/// выдумывает портфель. Что считать основой — у [`resolve_open_leverage`].
pub trait LeverageSnapshots {
    fn snapshot_for(&self, user: UserId) -> Option<&AccountLeverageSnapshot>;
}

/// Источник класса инструмента — **порт** (источник не назван владельцем).
///
/// `None` — класс символа неизвестен: разрешение пропустит планку класса и
/// останется на override клиента и основе портфеля (см. `resolve`). Выдумывать
/// класс нельзя.
pub trait InstrumentClasses {
    fn class_of(&self, instrument: u32) -> Option<InstrumentClass>;
}

/// Стаб приёмника снимков в памяти. Витрина (когда канал согласуют) кладёт сюда
/// снимок на счёт через [`Self::ingest`], терминал читает его на открытии. Пока
/// живого канала нет, тем же стабом пользуются проверки.
#[derive(Debug, Default)]
pub struct InMemoryLeverageSnapshots {
    by_user: HashMap<UserId, AccountLeverageSnapshot>,
}

impl InMemoryLeverageSnapshots {
    pub fn new() -> Self {
        Self::default()
    }

    /// Принять снимок на счёт — **точка подключения живой проводки site→terminal**.
    /// Последний пришедший снимок замещает прежний: снимок — это «как сейчас», а
    /// не лента.
    pub fn ingest(&mut self, user: UserId, snap: AccountLeverageSnapshot) {
        self.by_user.insert(user, snap);
    }
}

impl LeverageSnapshots for InMemoryLeverageSnapshots {
    fn snapshot_for(&self, user: UserId) -> Option<&AccountLeverageSnapshot> {
        self.by_user.get(&user)
    }
}

/// Стаб источника классов: таблица «инструмент → класс» в памяти. Источник в
/// бою не назван (CMS или терминал), поэтому класс кладётся явно через
/// [`Self::set`], а не выводится из символа.
#[derive(Debug, Default)]
pub struct InMemoryInstrumentClasses {
    by_instrument: HashMap<u32, InstrumentClass>,
}

impl InMemoryInstrumentClasses {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, instrument: u32, class: InstrumentClass) {
        self.by_instrument.insert(instrument, class);
    }
}

impl InstrumentClasses for InMemoryInstrumentClasses {
    fn class_of(&self, instrument: u32) -> Option<InstrumentClass> {
        self.by_instrument.get(&instrument).cloned()
    }
}

/// Разрешить плечо открытия из портов: снимок счёта + класс инструмента, по
/// приоритету Р-446 (override клиента › override класса › плечо портфеля), с
/// итогом снимком в позицию.
///
/// `fallback` — плечо, под которым открыть, когда **снимка нет или он негоден**:
/// честная основа, сегодня — плечо площадки (`Broker::leverage`). Это и есть
/// граница, которую нельзя изобретать живой проводкой здесь: пока снимок не
/// доезжает, терминал работает на площадочном плече, и это осознанное
/// состояние, а не умолчание.
///
/// Результат всегда положителен (`fallback.max(1)`), потому что идёт делителем
/// нотионала: отдать сюда ноль значило бы уронить расчёт в следующей же строке.
pub fn resolve_open_leverage(
    snapshots: &impl LeverageSnapshots,
    classes: &impl InstrumentClasses,
    user: UserId,
    instrument: u32,
    fallback: i128,
) -> i128 {
    let fallback = fallback.max(1);
    match snapshots.snapshot_for(user) {
        Some(snap) => {
            let class = classes.class_of(instrument);
            snap.resolve(class.as_ref()).unwrap_or(fallback)
        }
        None => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const USER: UserId = UserId(1);

    fn standard_snapshot() -> AccountLeverageSnapshot {
        let mut class_overrides = BTreeMap::new();
        class_overrides.insert(InstrumentClass::new("crypto"), 10);
        AccountLeverageSnapshot {
            portfolio_key: "standard".into(),
            base_leverage: 20,
            class_overrides,
            client_override: None,
            resolved_at: 0,
        }
    }

    /// Снимок есть, класс известен: берётся планка класса, а не площадочная основа.
    #[test]
    fn resolves_the_class_override_when_snapshot_and_class_are_known() {
        let mut snaps = InMemoryLeverageSnapshots::new();
        snaps.ingest(USER, standard_snapshot());
        let mut classes = InMemoryInstrumentClasses::new();
        classes.set(1, InstrumentClass::new("crypto"));

        assert_eq!(
            resolve_open_leverage(&snaps, &classes, USER, 1, 1),
            10,
            "crypto-override 10 старше площадочного плеча 1"
        );
    }

    /// Класс неизвестен — планка класса пропускается, в силе основа портфеля.
    #[test]
    fn falls_to_the_portfolio_base_when_class_is_unknown() {
        let mut snaps = InMemoryLeverageSnapshots::new();
        snaps.ingest(USER, standard_snapshot());
        let classes = InMemoryInstrumentClasses::new(); // класс символа не задан

        assert_eq!(
            resolve_open_leverage(&snaps, &classes, USER, 1, 1),
            20,
            "без класса действует плечо портфеля, а не площадки"
        );
    }

    /// Снимка нет — терминал остаётся на площадочном плече (честная основа).
    #[test]
    fn falls_back_to_platform_leverage_without_a_snapshot() {
        let snaps = InMemoryLeverageSnapshots::new();
        let classes = InMemoryInstrumentClasses::new();
        assert_eq!(
            resolve_open_leverage(&snaps, &classes, USER, 1, 7),
            7,
            "пока снимок не доезжает, в силе плечо площадки"
        );
    }

    /// Негодный снимок (неположительное плечо) не роняет открытие: берётся
    /// основа-fallback. Результат всегда положителен — он делит нотионал.
    #[test]
    fn a_broken_snapshot_falls_back_and_never_returns_nonpositive() {
        let mut snaps = InMemoryLeverageSnapshots::new();
        let mut bad = standard_snapshot();
        bad.base_leverage = 0;
        bad.class_overrides.clear();
        snaps.ingest(USER, bad);
        let classes = InMemoryInstrumentClasses::new();

        assert_eq!(resolve_open_leverage(&snaps, &classes, USER, 1, 5), 5);
        // И даже если сам fallback нулевой — наружу уходит минимум 1.
        assert_eq!(resolve_open_leverage(&snaps, &classes, USER, 1, 0), 1);
    }
}
