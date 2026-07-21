//! Конверсия денег между `i128` (домен) и `NUMERIC(38,0)` (БД).
//!
//! Почему это отдельный, придирчивый модуль: деньги в проекте — целые 128-битные
//! (`Amount(i128)` по ADR-006, `broker::Cents`), а `sqlx` отдаёт `NUMERIC` как
//! [`BigDecimal`]. Прямого маппинга нет, и любая небрежность здесь — это тихая потеря
//! копеек. Красная линия №1: при выходе за диапазон или ненулевой дробной части —
//! **ошибка, а не округление**.

use std::str::FromStr;

use bigdecimal::BigDecimal;

use crate::StoreError;

/// `i128` → `NUMERIC`. Точно: через десятичную запись, без плавающей арифметики.
pub fn to_numeric(v: i128) -> BigDecimal {
    // i128 всегда печатается как целое без экспоненты, поэтому разбор не может упасть.
    BigDecimal::from_str(&v.to_string()).expect("i128 всегда валидный BigDecimal")
}

/// `NUMERIC` → `i128`. Ошибка, если значение дробное или не влезает в `i128`.
pub fn from_numeric(bd: &BigDecimal) -> Result<i128, StoreError> {
    let truncated = bd.with_scale(0);
    if &truncated != bd {
        return Err(StoreError(format!("денежное значение не целое: {bd}")));
    }
    truncated
        .to_string()
        .parse::<i128>()
        .map_err(|_| StoreError(format!("денежное значение вне диапазона i128: {bd}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_value() {
        for v in [0i128, 1, -1, 12_345_678, -9_876_543_210, i128::MAX, i128::MIN] {
            assert_eq!(from_numeric(&to_numeric(v)).unwrap(), v, "round-trip {v}");
        }
    }

    #[test]
    fn fractional_is_rejected_not_rounded() {
        // Тихое округление здесь означало бы потерю копеек — только ошибка.
        let bd = BigDecimal::from_str("100.5").unwrap();
        assert!(from_numeric(&bd).is_err());
        let bd = BigDecimal::from_str("-0.01").unwrap();
        assert!(from_numeric(&bd).is_err());
    }

    #[test]
    fn trailing_zeros_are_still_integer() {
        // NUMERIC(38,0) может вернуться со scale > 0 и нулями в дробной части —
        // это по-прежнему целое, отвергать нельзя.
        let bd = BigDecimal::from_str("100.00").unwrap();
        assert_eq!(from_numeric(&bd).unwrap(), 100);
    }

    #[test]
    fn out_of_range_is_rejected() {
        let too_big = BigDecimal::from_str("170141183460469231731687303715884105728").unwrap(); // i128::MAX + 1
        assert!(from_numeric(&too_big).is_err());
    }
}
