#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct UserId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct AccountId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountKind {
    #[default]
    Demo,
    Real,
}

impl AccountKind {
    pub fn tag(self) -> &'static str {
        match self {
            AccountKind::Demo => "demo",
            AccountKind::Real => "real",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "demo" => Some(AccountKind::Demo),
            "real" => Some(AccountKind::Real),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountOrigin {
    #[default]
    Registration,
    Migration,
    Sandbox,
    Ipo,
}

impl AccountOrigin {
    pub fn tag(self) -> &'static str {
        match self {
            AccountOrigin::Registration => "registration",
            AccountOrigin::Migration => "migration",
            AccountOrigin::Sandbox => "sandbox",
            AccountOrigin::Ipo => "ipo",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "registration" => Some(AccountOrigin::Registration),
            "migration" => Some(AccountOrigin::Migration),
            "sandbox" => Some(AccountOrigin::Sandbox),
            "ipo" => Some(AccountOrigin::Ipo),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountStatus {
    #[default]
    Active,
    Suspended,
    Closed,
}

impl AccountStatus {
    pub fn tag(self) -> &'static str {
        match self {
            AccountStatus::Active => "active",
            AccountStatus::Suspended => "suspended",
            AccountStatus::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(AccountStatus::Active),
            "suspended" => Some(AccountStatus::Suspended),
            "closed" => Some(AccountStatus::Closed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClientCategory {
    #[default]
    Retail,
    Professional,
}

impl ClientCategory {
    pub fn tag(self) -> &'static str {
        match self {
            ClientCategory::Retail => "retail",
            ClientCategory::Professional => "professional",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "retail" => Some(ClientCategory::Retail),
            "professional" => Some(ClientCategory::Professional),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryAtOpen(pub ClientCategory);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryNow(pub ClientCategory);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Currency {
    code: [u8; 3],
    minor_units: u8,
}

const KNOWN: [(&str, u8); 5] = [("USD", 2), ("EUR", 2), ("JPY", 0), ("BHD", 3), ("KWD", 3)];

impl Currency {
    pub const USD: Currency = Currency {
        code: *b"USD",
        minor_units: 2,
    };

    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 3 || !b.iter().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        let code = [
            b[0].to_ascii_uppercase(),
            b[1].to_ascii_uppercase(),
            b[2].to_ascii_uppercase(),
        ];
        let as_str = std::str::from_utf8(&code).ok()?;
        KNOWN
            .iter()
            .find(|(c, _)| *c == as_str)
            .map(|(_, minor_units)| Currency {
                code,
                minor_units: *minor_units,
            })
    }

    pub fn from_storage(code: &str, minor_units: u8) -> Option<Self> {
        let c = Self::parse(code)?;
        (c.minor_units == minor_units).then_some(c)
    }

    pub fn code(&self) -> &str {
        std::str::from_utf8(&self.code).unwrap_or("???")
    }

    pub fn minor_units(&self) -> u8 {
        self.minor_units
    }

    pub fn minor_per_unit(&self) -> i128 {
        10i128.pow(self.minor_units as u32)
    }
}

impl Default for CategoryAtOpen {
    fn default() -> Self {
        CategoryAtOpen(ClientCategory::Retail)
    }
}

impl Default for Currency {
    fn default() -> Self {
        Currency::USD
    }
}

impl std::fmt::Display for Currency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccountProfile {
    pub owner: UserId,
    pub kind: AccountKind,
    pub origin: AccountOrigin,
    pub category_at_open: CategoryAtOpen,
    pub currency: Currency,
}

impl AccountProfile {
    pub fn legacy_demo(owner: UserId) -> Self {
        Self {
            owner,
            kind: AccountKind::Demo,
            origin: AccountOrigin::Registration,
            category_at_open: CategoryAtOpen(ClientCategory::Retail),
            currency: Currency::USD,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currency_is_three_letters_or_nothing() {
        assert_eq!(Currency::parse("usd"), Some(Currency::USD));
        assert_eq!(
            Currency::parse("EUR").map(|c| c.code().to_string()),
            Some("EUR".into())
        );
        assert_eq!(Currency::parse("US"), None, "две буквы — не код");
        assert_eq!(Currency::parse("USDT"), None, "четыре — тоже не код");
        assert_eq!(Currency::parse("U5D"), None, "цифра внутри — не код");
        assert_eq!(Currency::parse(""), None);
    }

    #[test]
    fn an_unknown_currency_cannot_be_opened() {
        assert_eq!(Currency::parse("XYZ"), None);
        assert_eq!(Currency::parse("BTC"), None, "и крипта тоже: её здесь нет");
    }

    #[test]
    fn precision_comes_from_the_currency_not_from_a_constant() {
        let cases = [("JPY", 0u8, 1i128), ("USD", 2, 100), ("BHD", 3, 1000)];
        for (code, units, per_unit) in cases {
            let c = Currency::parse(code).unwrap_or_else(|| panic!("{code} обязана быть известна"));
            assert_eq!(c.minor_units(), units, "{code}");
            assert_eq!(c.minor_per_unit(), per_unit, "{code}");
        }
    }

    #[test]
    fn stored_precision_must_agree_with_the_table() {
        assert_eq!(Currency::from_storage("USD", 2), Some(Currency::USD));
        assert_eq!(
            Currency::from_storage("USD", 3),
            None,
            "доллар с тремя знаками — испорченная строка, а не валюта"
        );
        assert_eq!(Currency::from_storage("JPY", 2), None);
        assert_eq!(Currency::from_storage("XYZ", 2), None);
    }

    #[test]
    fn machine_names_round_trip() {
        for k in [AccountKind::Demo, AccountKind::Real] {
            assert_eq!(AccountKind::parse(k.tag()), Some(k));
        }
        for c in [ClientCategory::Retail, ClientCategory::Professional] {
            assert_eq!(ClientCategory::parse(c.tag()), Some(c));
        }
    }

    #[test]
    fn an_unknown_name_is_refused_not_defaulted() {
        assert_eq!(AccountKind::parse("демо"), None);
        assert_eq!(AccountKind::parse(""), None);
        assert_eq!(ClientCategory::parse("vip"), None);
    }

    #[test]
    fn origin_and_status_round_trip_and_refuse_strangers() {
        for o in [
            AccountOrigin::Registration,
            AccountOrigin::Migration,
            AccountOrigin::Sandbox,
            AccountOrigin::Ipo,
        ] {
            assert_eq!(AccountOrigin::parse(o.tag()), Some(o));
        }
        for st in [
            AccountStatus::Active,
            AccountStatus::Suspended,
            AccountStatus::Closed,
        ] {
            assert_eq!(AccountStatus::parse(st.tag()), Some(st));
        }
        assert_eq!(
            AccountOrigin::parse("ico"),
            None,
            "похожее имя — не то же самое"
        );
        assert_eq!(AccountStatus::parse("blocked"), None);
    }

    #[test]
    fn the_two_categories_are_different_types() {
        let at_open = CategoryAtOpen(ClientCategory::Retail);
        let now = CategoryNow(ClientCategory::Professional);

        assert_eq!(at_open.0, ClientCategory::Retail);
        assert_eq!(now.0, ClientCategory::Professional);
    }

    #[test]
    fn defaults_describe_the_accounts_that_already_exist() {
        let p = AccountProfile::legacy_demo(UserId(7));
        assert_eq!(p.owner, UserId(7));
        assert_eq!(p.kind, AccountKind::Demo);
        assert_eq!(p.category_at_open, CategoryAtOpen(ClientCategory::Retail));
        assert_eq!(p.origin, AccountOrigin::Registration);
        assert_eq!(p.currency, Currency::USD);
        assert_eq!(
            p,
            AccountProfile {
                owner: UserId(7),
                ..Default::default()
            }
        );
    }
}
