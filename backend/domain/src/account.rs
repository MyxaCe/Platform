//! Торговый счёт: кто им владеет, какого он рода, чьей категории и в какой валюте.
//!
//! # Почему четыре поля заводятся вместе (TRM-44)
//!
//! Решение владельца 24.10. Каждое добавленное **позже** — это миграция живых
//! денежных данных: сегодня счетов единицы, через месяц тысячи. Поэтому модель
//! счёта описывается целиком сразу, даже там, где второе значение поля ещё
//! негде получить.
//!
//! Здесь только **словарь** — ни одного правила. Правила (стартовый баланс по
//! роду счёта, допуск к торговле) живут там, где применяются: в брокере и в
//! шлюзе. Словарь в `domain` затем и существует, чтобы одно понятие не
//! называлось по-разному в трёх крейтах.

/// Глобально уникальный id пользователя — **человека**, а не его счёта.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct UserId(pub u64);

/// Идентификатор **счёта**.
///
/// Отдельный тип от [`UserId`], хотя сегодня у человека счёт один и значения
/// совпадают. Разделены **типами**, а не договорённостью: у одного владельца
/// скоро будет демо-счёт и реальный, а при мультивалютности — по счёту на
/// валюту. Когда это произойдёт, компилятор покажет каждое место, где «счёт» и
/// «пользователь» перепутаны; соглашение «помним, что это разные вещи» не
/// покажет ничего.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct AccountId(pub u64);

/// Род счёта: бумажные деньги или настоящие.
///
/// **Разделены намеренно и жёстко.** Это не оттенок одного и того же: на
/// реальном счёте ошибка стоит денег клиента, на демо — ничего. Код, который не
/// может ответить, с каким из двух он работает, не имеет права трогать ни один.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountKind {
    /// Бумажные деньги на реальных ценах (ADR-013, ADR-014). Единственный род,
    /// существовавший до TRM-44, — поэтому он же умолчание для счетов,
    /// заведённых раньше: это факт их истории, а не догадка.
    #[default]
    Demo,
    /// Настоящие средства клиента.
    Real,
}

impl AccountKind {
    /// Машинное имя для журнала, хранилища и границ.
    pub fn tag(self) -> &'static str {
        match self {
            AccountKind::Demo => "demo",
            AccountKind::Real => "real",
        }
    }

    /// Разобрать машинное имя. `None` — имя неизвестно, и это **отказ**, а не
    /// повод подставить умолчание: род счёта решает, настоящие ли это деньги.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "demo" => Some(AccountKind::Demo),
            "real" => Some(AccountKind::Real),
            _ => None,
        }
    }
}

/// Откуда счёт взялся.
///
/// Отдельная ось от [`AccountKind`], и это решение владельца: род отвечает
/// **чем счёт является**, происхождение — **откуда взялся**. Пока «песочница»
/// стояла в роде, «демо-счёт из песочницы» выразить было нечем, а «песочница
/// неизвестного рода» — можно. Теперь наоборот, и это правильный наоборот.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountOrigin {
    /// Обычная регистрация в кабинете.
    #[default]
    Registration,
    /// Перенесён из прежнего состояния платформы.
    Migration,
    /// Стенд и показы. Деньги такого счёта в отчётность не входят.
    Sandbox,
    /// Отложенный модуль размещения (владелец добавил 24.10).
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

/// Состояние счёта — **меняется**, в отличие от остального профиля.
///
/// Поэтому и лежит отдельно от [`AccountProfile`]: тот неизменен с момента
/// заведения, а это величина жизненного цикла. Сложить их в одну структуру
/// значило бы сказать, что род счёта меняется так же легко, как его состояние.
///
/// Состояние **счёта**, а не клиента: блокировка клиента гасит все его счета,
/// но счёт может быть закрыт и у действующего клиента.
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

/// Категория клиента — розничный или профессиональный.
///
/// От неё зависит объём защиты, которую обязан дать брокер: плечо,
/// предупреждения, защита от отрицательного баланса. Поле заводится сейчас,
/// когда различать категории ещё нечем: правила юрисдикции появятся позже, а
/// миграция живых счетов обойдётся дороже пустого поля.
///
/// **Умолчание — розничный**, и это не нейтральный выбор, а осознанно
/// консервативный: розничному положено больше защиты. Ошибиться в сторону
/// большей защиты дёшево, в обратную — нет.
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

/// Категория, **действовавшая на момент открытия счёта**.
///
/// # Почему это отдельный тип, а не просто поле
///
/// Решение владельца 24.10: у правила **два входа с разным временем, и оба
/// живые одновременно**.
///
/// - уже открытая позиция судится по категории **на момент своего открытия** —
///   перевод человека в профессиональные не вправе задним числом снять защиту
///   с того, что он открывал розничным;
/// - новая сделка судится по категории **сейчас**.
///
/// Это **не** пара «историческое и текущее», как у цены выхода и уровня: там
/// живая величина одна. Здесь действуют обе, и перепутать их значит либо снять
/// защиту с того, кому она полагалась, либо оставить её тому, кто от неё
/// отказался.
///
/// Поэтому они разные **типы**, а не два значения одного. Неявного превращения
/// между ними нет: функция, которой нужна категория на момент открытия, не
/// примет текущую, и компилятор покажет каждое такое место. Соглашение «помним,
/// что это разные вещи» не покажет ни одного.
///
/// Цена названа честно: там, где нужны обе, придётся распаковать обе явно. Это
/// единственное место, где перепутать было бы легко, и теперь оно видно в коде.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryAtOpen(pub ClientCategory);

/// Категория клиента **сейчас**. Приходит извне на каждое решение, в профиле
/// счёта не хранится: она свойство клиента, а не счёта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryNow(pub ClientCategory);

/// Валюта счёта — трёхбуквенный код.
///
/// **Атрибут счёта, а не допущение кода.** До TRM-44 валюта нигде не хранилась:
/// суммы приходили из источника, а то, что они в долларах, было
/// договорённостью в голове. Для второй страны это ошибка не в отображении, а
/// **в деньгах**: сложить центы доллара с центами евро можно, и никто не
/// заметит.
///
/// Хранится тремя байтами, а не `String`: код валюты либо трёхбуквенный, либо
/// это не код валюты, и проверить это один раз на входе дешевле, чем доверять
/// каждому вызывающему.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Currency {
    code: [u8; 3],
    /// Сколько знаков после запятой у этой валюты.
    ///
    /// **Точность едет рядом с кодом, а не выводится при чтении.** Суммы
    /// хранятся целыми в наименьшей единице, и смысл записанного числа задаёт
    /// именно это значение: 1000 это десять долларов, тысяча иен или одна целая
    /// динара. Если бы точность бралась из справочника при каждом чтении,
    /// правка справочника молча переписала бы смысл всех прошлых сумм.
    minor_units: u8,
}

/// Валюты, которые терминал умеет завести, и их точность.
///
/// **Перечень, а не догадка.** Код валюты, которого здесь нет, завести нельзя:
/// точность выдумать невозможно, а ошибка в ней — это ошибка в деньгах ровно в
/// `10^n` раз. Новая валюта добавляется сюда, а не «подставляется двойкой».
///
/// Значения взяты из ISO 4217. Ноль, два и три знака представлены намеренно:
/// на двух вариантах любая проверка проходится завязкой «ноль или не ноль».
const KNOWN: [(&str, u8); 5] = [
    ("USD", 2),
    ("EUR", 2),
    ("JPY", 0), // иена без дробной части
    ("BHD", 3), // динар Бахрейна — три знака
    ("KWD", 3),
];

impl Currency {
    /// Валюта всех существовавших до TRM-44 счетов. Не умолчание «на всякий
    /// случай», а факт их истории: другой валюты в терминале не было.
    pub const USD: Currency = Currency {
        code: *b"USD",
        minor_units: 2,
    };

    /// Завести валюту по коду. `None` — код не трёхбуквенный **или валюта нам
    /// неизвестна**, а значит её точность неизвестна тоже.
    ///
    /// Прежняя версия принимала любые три буквы: тогда валюта была лишь ярлыком и
    /// на счёт деньги не влияла. С появлением точности это перестало быть
    /// безобидным — принять `XYZ` значило бы завести счёт, суммы на котором
    /// нечем истолковать.
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

    /// Поднять валюту из хранилища: код **и записанная рядом с ним** точность.
    ///
    /// Расхождение с перечнем — **отказ**, а не выбор одной из двух правд. Если
    /// в строке счёта написано «USD, три знака», то либо испорчена строка, либо
    /// у нас поменялся перечень; в обоих случаях прошлые суммы этого счёта
    /// истолкованы не будут, и подставлять любое из двух значило бы угадывать
    /// размер чужих денег.
    pub fn from_storage(code: &str, minor_units: u8) -> Option<Self> {
        let c = Self::parse(code)?;
        (c.minor_units == minor_units).then_some(c)
    }

    pub fn code(&self) -> &str {
        // Байты проверены при разборе: три ASCII-буквы — всегда валидный UTF-8.
        std::str::from_utf8(&self.code).unwrap_or("???")
    }

    /// Сколько знаков после запятой. Множитель наименьшей единицы — `10^n`.
    pub fn minor_units(&self) -> u8 {
        self.minor_units
    }

    /// Во сколько наименьших единиц укладывается одна единица валюты.
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

/// Неизменная часть счёта: кем заведён, какого рода, чьей категории, в какой
/// валюте.
///
/// Все четыре поля устанавливаются **при заведении** и обычными операциями не
/// меняются. Смена рода счёта или валюты — это не правка поля, а другой счёт:
/// деньги, посчитанные в одной валюте, не становятся задним числом суммой в
/// другой.
///
/// **Времени заведения здесь нет намеренно.** Оно уже есть — это `at_ms`
/// записи о заведении счёта в журнале. Второе поле с тем же смыслом однажды
/// разойдётся с первым, и тогда непонятно, какое из двух правда.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccountProfile {
    /// Чей это счёт. Сегодня совпадает с ключом, потому что счёт у человека
    /// один; поле существует, чтобы перестать совпадать без миграции.
    pub owner: UserId,
    pub kind: AccountKind,
    pub origin: AccountOrigin,
    /// Категория, действовавшая при открытии. **Снимок, а не текущая**: по ней
    /// считаются защиты уже открытых позиций.
    pub category_at_open: CategoryAtOpen,
    pub currency: Currency,
}

impl AccountProfile {
    /// Профиль счёта, какими были все счета до TRM-44: демо, розничный, доллар.
    ///
    /// Отдельным конструктором с именем, а не `Default::default()` у места
    /// вызова: «такими они и были» — утверждение об истории, и его надо читать,
    /// а не выводить из того, что поле не заполнили.
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

    /// **Незнакомая валюта не заводится.** Точность выдумать нельзя, а ошибка в
    /// ней — это ошибка в деньгах ровно в `10^n` раз.
    #[test]
    fn an_unknown_currency_cannot_be_opened() {
        assert_eq!(Currency::parse("XYZ"), None);
        assert_eq!(Currency::parse("BTC"), None, "и крипта тоже: её здесь нет");
    }

    /// Точность — у каждой своя, и **три разные** в перечне намеренно: на двух
    /// любая проверка проходится завязкой «ноль или не ноль».
    #[test]
    fn precision_comes_from_the_currency_not_from_a_constant() {
        let cases = [("JPY", 0u8, 1i128), ("USD", 2, 100), ("BHD", 3, 1000)];
        for (code, units, per_unit) in cases {
            let c = Currency::parse(code).unwrap_or_else(|| panic!("{code} обязана быть известна"));
            assert_eq!(c.minor_units(), units, "{code}");
            assert_eq!(c.minor_per_unit(), per_unit, "{code}");
        }
    }

    /// **Записанная точность обязана сходиться с перечнем.**
    ///
    /// Расхождение — отказ, а не выбор одной из двух правд: прошлые суммы
    /// такого счёта истолковать нечем, и подставить любое значило бы угадать
    /// размер чужих денег.
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

    /// Машинные имена ездят в журнал и в хранилище: round-trip обязан сходиться,
    /// иначе поднятый счёт окажется другого рода, чем записанный.
    #[test]
    fn machine_names_round_trip() {
        for k in [AccountKind::Demo, AccountKind::Real] {
            assert_eq!(AccountKind::parse(k.tag()), Some(k));
        }
        for c in [ClientCategory::Retail, ClientCategory::Professional] {
            assert_eq!(ClientCategory::parse(c.tag()), Some(c));
        }
    }

    /// Неизвестное имя — **отказ**, а не умолчание. Род счёта решает, настоящие
    /// ли это деньги; подставить сюда «демо» значит разрешить торговать
    /// настоящими как бумажными.
    #[test]
    fn an_unknown_name_is_refused_not_defaulted() {
        assert_eq!(AccountKind::parse("демо"), None);
        assert_eq!(AccountKind::parse(""), None);
        assert_eq!(ClientCategory::parse("vip"), None);
    }

    /// Происхождение и состояние — закрытые перечни: неизвестное имя **отказ**.
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

    /// **Две категории не подменяют друг друга.**
    ///
    /// Проверка на типах: её ценность не в утверждении ниже, а в том, что
    /// обратное **не компилируется**. Оставлено строкой, чтобы следующий, кто
    /// захочет «упростить» их до одного `ClientCategory`, прочитал, зачем их
    /// два, прежде чем сливать.
    #[test]
    fn the_two_categories_are_different_types() {
        let at_open = CategoryAtOpen(ClientCategory::Retail);
        let now = CategoryNow(ClientCategory::Professional);

        // Нужное значение достаётся явно — и это та самая названная цена:
        // в точке, где нужны обе, видно, какая из них какая.
        assert_eq!(at_open.0, ClientCategory::Retail);
        assert_eq!(now.0, ClientCategory::Professional);

        // А вот это не скомпилируется, и в том весь смысл:
        //     let _: CategoryAtOpen = now;
        //     fn protect(c: CategoryAtOpen) {}  protect(now);
    }

    /// Умолчания названы и проверены: они описывают **историю существующих
    /// счетов**, а не удобство. Демо, розничный, доллар — другого не было.
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
