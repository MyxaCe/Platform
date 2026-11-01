//! Снимок плеча портфеля и его разрешение (Р-446, TRM-53 шаг 3).
//!
//! **Плечо — свойство портфеля** (Р-446), не инструмента и не площадки. Снимок
//! строит сторона витрины (site-platform, Веха 1, контракт `AccountLeverageSnapshot`)
//! и присылает его в терминал; здесь — структура снимка один в один и правило
//! его **разрешения** в одно число на момент открытия позиции.
//!
//! Что здесь СОЗНАТЕЛЬНО не решается:
//!
//! - **живой канал доставки site→terminal** на 01.11 ещё не согласован (витрина
//!   снимок строит, но не шлёт). Приём снимка в терминал абстрагирован портом
//!   (см. `broker::leverage`); живую проводку изобретать нельзя — она вынесена
//!   штабу как контракт. Здесь только форма снимка и его смысл;
//! - **откуда берётся класс конкретного символа** (CMS или терминал) — открытый
//!   вопрос владельцу. Поэтому класс здесь — строковый ключ, принимаемый как
//!   ВХОД, а не закрытый перечень: выдумывать источник на этой стороне нельзя.
//!
//! Разрешение (`resolve`) живёт на самом снимке намеренно: приоритет уровней —
//! это **смысл контракта**, а не правило брокера, ровно как точность у
//! [`crate::account::Currency`] едет с кодом валюты, а не выводится при чтении.

use std::collections::BTreeMap;

/// Класс инструмента: ключ переопределения плеча внутри портфеля (Р-446).
///
/// **Строковый ключ, а не `enum`, — намеренно.** Откуда терминал узнаёт класс
/// символа (CMS или сам терминал) — открытый вопрос владельцу; пока источник не
/// назван, класс принимается как вход, а не зашивается перечнем. `classOverrides`
/// контракта — это отображение такого ключа в плечо (`{"crypto":10,…}`), и ключ
/// обязан совпасть с присланным витриной байт в байт: `"crypto"` и `"Crypto"` —
/// разные классы, и сверять их значение здесь не вправе.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstrumentClass(pub String);

impl InstrumentClass {
    pub fn new(s: impl Into<String>) -> Self {
        InstrumentClass(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Снимок плеча портфеля на момент разрешения витриной (контракт site-platform,
/// Веха 1, `AccountLeverageSnapshot`).
///
/// **Снимок, как категория клиента при открытии счёта** (Р-446, семья Р-293 про
/// снимки): терминал разрешает плечо по присланному, а не ходит за портфелем в
/// витрину на каждое открытие. Смена портфеля задним числом не трогает уже
/// открытые позиции — у них своё плечо, записанное при открытии.
///
/// Поля повторяют контракт один в один; имена в коде — в змеином регистре,
/// соответствие к `camelCase` контракта названо у каждого.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLeverageSnapshot {
    /// `portfolioKey` — ключ портфеля, к которому относится снимок. Терминал его
    /// не толкует, а хранит: по нему снимок сверяется с журналом витрины.
    pub portfolio_key: String,
    /// `baseLeverage` — плечо портфеля, основа. Берётся, когда ни override
    /// клиента, ни override класса ничего не сказали.
    pub base_leverage: i128,
    /// `classOverrides` — переопределения по классам инструмента: крипте нужна
    /// своя планка (Р-446). **Не основа, а переопределение** — отсутствие ключа
    /// значит «для этого класса особой планки нет», а не «плечо ноль».
    pub class_overrides: BTreeMap<InstrumentClass, i128>,
    /// `clientOverride` — переопределение конкретному клиенту, по заявке с
    /// записью в журнал на стороне витрины (Р-446). `None` — заявки не было.
    pub client_override: Option<i128>,
    /// `resolvedAt` — момент, на который витрина разрешила снимок (мс эпохи).
    /// Терминал НЕ толкует его как порог свежести: это отметка источника, а
    /// свежесть — отдельный разговор, которого контракт пока не ведёт.
    pub resolved_at: i64,
}

impl AccountLeverageSnapshot {
    /// Разрешить плечо для класса инструмента по приоритету Р-446:
    /// **override клиента › override класса в портфеле › плечо портфеля**.
    ///
    /// Побеждает **первый названный** уровень, а не первый валидный: если у
    /// клиента есть override, решает он — даже когда под ним лежит планка класса.
    /// Иначе испорченное число у клиента молча подменилось бы планкой класса, и
    /// клиент торговал бы не на том плече, на которое ему выписали заявку.
    ///
    /// `class` — `None`, когда класс символа неизвестен (источник класса —
    /// открытый вопрос владельцу): тогда override класса пропускается, и в силе
    /// остаются override клиента и основа. Выдумывать класс нельзя, а ронять
    /// открытие из-за неизвестного класса — тоже: основа портфеля есть всегда.
    ///
    /// `None` на выходе — **снимок негоден** (выбранный уровень дал
    /// неположительное плечо). Плечо делит нотионал: ноль уронил бы расчёт
    /// делением, отрицательное дало бы отрицательную маржу — ту же защиту несёт
    /// `Broker::set_leverage`. Открывать по негодному снимку нельзя; что делать
    /// вместо — решает вызывающий (см. `broker::leverage::resolve_open_leverage`).
    pub fn resolve(&self, class: Option<&InstrumentClass>) -> Option<i128> {
        let chosen = self
            .client_override
            .or_else(|| class.and_then(|c| self.class_overrides.get(c).copied()))
            .unwrap_or(self.base_leverage);
        (chosen > 0).then_some(chosen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> AccountLeverageSnapshot {
        let mut class_overrides = BTreeMap::new();
        class_overrides.insert(InstrumentClass::new("crypto"), 10);
        class_overrides.insert(InstrumentClass::new("forex"), 100);
        AccountLeverageSnapshot {
            portfolio_key: "standard".into(),
            base_leverage: 20,
            class_overrides,
            client_override: None,
            resolved_at: 0,
        }
    }

    /// Override класса старше основы портфеля: крипте — своя планка 10, а не 20.
    #[test]
    fn class_override_beats_the_portfolio_base() {
        let s = snap();
        assert_eq!(s.resolve(Some(&InstrumentClass::new("crypto"))), Some(10));
        assert_eq!(s.resolve(Some(&InstrumentClass::new("forex"))), Some(100));
    }

    /// Класс без своей планки падает на основу портфеля, а не на ноль.
    #[test]
    fn a_class_without_an_override_falls_to_the_base() {
        let s = snap();
        assert_eq!(
            s.resolve(Some(&InstrumentClass::new("stocks"))),
            Some(20),
            "для класса без планки действует плечо портфеля"
        );
    }

    /// Неизвестный класс (источник класса не назван) — тоже основа, а не отказ.
    #[test]
    fn an_unknown_class_falls_to_the_base_not_a_refusal() {
        let s = snap();
        assert_eq!(s.resolve(None), Some(20));
    }

    /// Override клиента старше всего: и планки класса, и основы.
    #[test]
    fn client_override_beats_class_and_base() {
        let mut s = snap();
        s.client_override = Some(5);
        assert_eq!(
            s.resolve(Some(&InstrumentClass::new("crypto"))),
            Some(5),
            "заявка клиента старше планки класса"
        );
        assert_eq!(s.resolve(None), Some(5));
    }

    /// Названный уровень побеждает, даже если он испорчен: клиентский override 0
    /// не проваливается тихо в планку класса — снимок признаётся негодным.
    #[test]
    fn a_present_but_broken_client_override_does_not_silently_fall_through() {
        let mut s = snap();
        s.client_override = Some(0);
        assert_eq!(
            s.resolve(Some(&InstrumentClass::new("crypto"))),
            None,
            "испорченный override клиента — негодный снимок, а не повод взять планку класса"
        );
    }

    /// Неположительное плечо на выбранном уровне делает снимок негодным: плечо
    /// делит нотионал.
    #[test]
    fn non_positive_leverage_is_refused() {
        let mut s = snap();
        s.base_leverage = 0;
        assert_eq!(s.resolve(None), None, "ноль уронил бы расчёт делением");
        s.base_leverage = -10;
        assert_eq!(s.resolve(None), None, "отрицательное дало бы отрицательную маржу");
    }
}
