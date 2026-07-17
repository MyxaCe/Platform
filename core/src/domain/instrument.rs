//! Инструмент (торговая пара) и реестр инструментов (ADR-005).
//!
//! Инструмент задаёт правила своей книги: точность, шаг цены (`tick_size`), шаг объёма
//! (`lot_size`), минимальный объём (`min_qty`). Активы `base`/`quote` — данные для будущего
//! Ledger'а (Фаза 2). Реестр — конфиг-состояние движка; инструмент регистрируется до торгов.

use std::collections::HashMap;

use crate::domain::money::{Price, Qty};

/// Идентификатор актива (BTC, USDT, …). Пока опаковый — полноценный реестр активов будет позже.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssetId(pub u32);

/// Идентификатор инструмента (торговой пары).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstrumentId(pub u32);

/// Торговый инструмент и его параметры.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    pub id: InstrumentId,
    pub symbol: String,
    /// Базовый актив (то, что покупают/продают).
    pub base: AssetId,
    /// Котируемый актив (в чём цена).
    pub quote: AssetId,
    pub price_decimals: u8,
    pub qty_decimals: u8,
    /// Шаг цены: цена лимитной заявки должна быть кратна ему.
    pub tick_size: Price,
    /// Шаг объёма: объём заявки должен быть кратен ему.
    pub lot_size: Qty,
    /// Минимальный объём заявки.
    pub min_qty: Qty,
}

/// Реестр инструментов. Конфиг-состояние: заполняется до начала торгов.
#[derive(Debug, Default)]
pub struct InstrumentRegistry {
    by_id: HashMap<InstrumentId, Instrument>,
}

impl InstrumentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Зарегистрировать инструмент. Параметры валидируются здесь (это конфиг-граница):
    /// шаги и минимум обязаны быть строго положительными.
    pub fn register(&mut self, inst: Instrument) {
        assert!(inst.tick_size.0 > 0, "tick_size должен быть > 0 ({})", inst.symbol);
        assert!(inst.lot_size.0 > 0, "lot_size должен быть > 0 ({})", inst.symbol);
        assert!(inst.min_qty.0 > 0, "min_qty должен быть > 0 ({})", inst.symbol);
        assert!(
            inst.min_qty.0 % inst.lot_size.0 == 0,
            "min_qty должен быть кратен lot_size ({})",
            inst.symbol
        );
        self.by_id.insert(inst.id, inst);
    }

    pub fn get(&self, id: InstrumentId) -> Option<&Instrument> {
        self.by_id.get(&id)
    }

    pub fn contains(&self, id: InstrumentId) -> bool {
        self.by_id.contains_key(&id)
    }
}
