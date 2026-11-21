#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dec {
    pub m: i128,
    pub s: u32,
}

impl Dec {
    pub fn parse(t: &str) -> Option<Dec> {
        let t = t.trim();
        let (int, frac) = match t.split_once('.') {
            Some((i, f)) => (i, f),
            None => (t, ""),
        };
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        if !int.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        if int.len() + frac.len() > 30 {
            return None;
        }
        let digits = format!("{int}{frac}");
        let m: i128 = if digits.is_empty() {
            0
        } else {
            digits.parse().ok()?
        };
        Some(
            Dec {
                m,
                s: frac.len() as u32,
            }
            .normalized(),
        )
    }

    pub fn normalized(self) -> Dec {
        let mut d = self;
        while d.s > 0 && d.m % 10 == 0 {
            d.m /= 10;
            d.s -= 1;
        }
        d
    }

    pub fn decimals(self) -> u32 {
        self.normalized().s
    }

    pub fn is_positive(self) -> bool {
        self.m > 0
    }

    pub fn mul(self, o: Dec) -> Option<Dec> {
        Some(
            Dec {
                m: self.m.checked_mul(o.m)?,
                s: self.s.checked_add(o.s)?,
            }
            .normalized(),
        )
    }

    pub fn raw(self, dec: u32) -> Option<i64> {
        let d = self.normalized();
        let v = if d.s <= dec {
            d.m.checked_mul(10i128.checked_pow(dec - d.s)?)?
        } else {
            let div = 10i128.checked_pow(d.s - dec)?;
            if d.m % div != 0 {
                return None;
            }
            d.m / div
        };
        i64::try_from(v).ok()
    }
}

impl std::fmt::Display for Dec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let d = self.normalized();
        if d.s == 0 {
            return write!(f, "{}", d.m);
        }
        let p = 10i128.pow(d.s);
        write!(f, "{}.{:0width$}", d.m / p, d.m % p, width = d.s as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes() {
        assert_eq!(Dec::parse("0.01000000"), Some(Dec { m: 1, s: 2 }));
        assert_eq!(Dec::parse("100000"), Some(Dec { m: 100000, s: 0 }));
        assert_eq!(Dec::parse("1.5").unwrap().to_string(), "1.5");
        for bad in ["", ".", "1e5", "-1", "1,5", "abc", "1.2.3"] {
            assert_eq!(Dec::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn raw_is_exact_or_refused() {
        let tick = Dec::parse("0.00001").unwrap();
        assert_eq!(tick.raw(5), Some(1));
        assert_eq!(tick.raw(4), None);
        let lot = Dec::parse("0.01")
            .unwrap()
            .mul(Dec::parse("100000").unwrap())
            .unwrap();
        assert_eq!(lot.raw(0), Some(1000));
        assert_eq!(lot.decimals(), 0);
    }
}
