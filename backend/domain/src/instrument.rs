use std::collections::HashMap;

use crate::money::{Price, Qty};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssetId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstrumentId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    pub id: InstrumentId,
    pub symbol: String,
    pub base: AssetId,
    pub quote: AssetId,
    pub price_decimals: u8,
    pub qty_decimals: u8,
    pub tick_size: Price,
    pub lot_size: Qty,
    pub min_qty: Qty,
}

#[derive(Debug, Default)]
pub struct InstrumentRegistry {
    by_id: HashMap<InstrumentId, Instrument>,
}

impl InstrumentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, inst: Instrument) {
        assert!(
            inst.tick_size.0 > 0,
            "tick_size должен быть > 0 ({})",
            inst.symbol
        );
        assert!(
            inst.lot_size.0 > 0,
            "lot_size должен быть > 0 ({})",
            inst.symbol
        );
        assert!(
            inst.min_qty.0 > 0,
            "min_qty должен быть > 0 ({})",
            inst.symbol
        );
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
