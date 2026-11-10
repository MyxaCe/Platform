//! # broker
//!
//! Бумажный брокер (ADR-014): счета с виртуальным балансом, позиции Long/Short с SL/TP, отложенные
//! (лимитные) ордера с триггером по цене, история закрытых сделок, маржа и P&L на реальных ценах.
//! Деньги — в **центах** (`i128`). Реальных денег нет.

use std::collections::{HashMap, HashSet};

use domain::account::UserId;
/// Реэкспорт словаря счёта: шлюз работает с ним через брокера, а не
/// подключает `domain` ради одних имён.
pub use domain::account::{
    AccountKind, AccountOrigin, AccountProfile, CategoryAtOpen, CategoryNow, ClientCategory,
    Currency,
};

pub mod journal;

use journal::{CloseCause, EventKind, RejectReason};

/// Деньги в центах (1/100 USD).
/// Денежная величина — целое в **наименьшей единице валюты счёта** (TRM-55).
///
/// Прежнее имя `Cents` сохранено синонимом ради чужих вызовов, но оно лжёт:
/// «цент» верен только для валют с двумя знаками. Для иены наименьшая единица —
/// сама иена, для динара Бахрейна — одна тысячная.
pub type Minor = i128;

/// Прежнее имя [`Minor`]. **Не использовать в новом коде**: оно утверждает
/// точность, которой у валюты может не быть.
pub type Cents = Minor;

/// Сторона позиции/ордера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosSide {
    Long,
    Short,
}

/// Стоимость `price · qty` в центах: `price_raw · qty_raw · 100 / 10^(pd+qd)`.
/// Стоимость объёма в **наименьших единицах валюты счёта** (TRM-55).
///
/// # Здесь был зашит цент
///
/// Прежняя редакция умножала на `100` — то есть утверждала, что у валюты счёта
/// ровно два знака. Для счёта в иенах это завысило бы каждую маржу и каждый
/// P&L **в сто раз**, для динара — занизило бы в десять. Константа выглядела
/// безобидным переводом «в копейки» ровно потому, что валюта была одна.
///
/// Теперь множитель приходит от валюты счёта. «Целое» означает наименьшую
/// единицу **этой** валюты, а не цент.
fn value_minor(price_raw: i64, qty_raw: i64, pd: u8, qd: u8, minor_per_unit: i128) -> Minor {
    price_raw as i128 * qty_raw as i128 * minor_per_unit / 10i128.pow(pd as u32 + qd as u32)
}

/// Модель ведения позиций счёта (ADR-025).
///
/// Свойство **счёта**, а не позиции: неттинг — это отношение между позициями, и
/// хранить его на отдельной позиции значит хранить на неверном уровне (разбор в ADR).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PositionModel {
    /// Встречная сделка открывает вторую позицию. Единственная модель до 2026-09-04,
    /// поэтому она же — умолчание для счетов, заведённых раньше: это факт их истории,
    /// а не догадка.
    #[default]
    Hedging,
    /// Счёт держит не больше одной позиции на инструмент: доливка усредняет вход,
    /// встречная сделка сворачивает позицию.
    Netting,
}

// ===================== Денежная математика неттинга =========================
//
// Вынесено в чистые функции сознательно: ветвление «свернуть или открыть» — скучная
// часть, а вот усреднение входа и реализованный P&L при ЧАСТИЧНОМ сворачивании — то
// место, где ошибка стоит денег и не падает. Считается отдельно, проверяется отдельно.

/// Средневзвешенная цена входа при доливке к позиции того же направления.
///
/// Округление — к ближайшему, а не отбрасыванием: отбрасывание systematically
/// смещает цену входа в одну сторону, то есть на длинной дистанции дарит или отнимает
/// деньги. Половина округляется от нуля.
pub fn merged_entry(qty_a: i64, entry_a: i64, qty_b: i64, entry_b: i64) -> i64 {
    let total = qty_a as i128 + qty_b as i128;
    if total == 0 {
        return entry_a;
    }
    let sum = entry_a as i128 * qty_a as i128 + entry_b as i128 * qty_b as i128;
    let half = total / 2;
    let rounded = if sum >= 0 {
        (sum + half) / total
    } else {
        (sum - half) / total
    };
    rounded as i64
}

/// Реализованный P&L при сворачивании `qty` из позиции стороны `side`.
///
/// Отдельно от `Position::unrealized`, потому что объём здесь **частичный**: берётся
/// не весь объём позиции, а свёрнутая часть.
pub fn realized_on_close(
    side: PosSide,
    entry: i64,
    exit: i64,
    qty: i64,
    pd: u8,
    qd: u8,
    minor_per_unit: i128,
) -> Minor {
    let diff = match side {
        PosSide::Long => exit - entry,
        PosSide::Short => entry - exit,
    };
    value_minor(diff, qty, pd, qd, minor_per_unit)
}

/// Маржа под объём `qty` по цене `entry` при плече `lev` — та же формула, что при
/// открытии. Отдельной функцией, чтобы неттинг и открытие не разъехались.
pub fn margin_for(entry: i64, qty: i64, pd: u8, qd: u8, lev: i128, minor_per_unit: i128) -> Minor {
    value_minor(entry, qty, pd, qd, minor_per_unit) / lev.max(1)
}

// ===================== Тариф счёта: наценка и комиссия ======================
//
// Доход площадки (решение владельца 10.11): наценка на спред (портфель
// Standard) и комиссия за сделку (Pro / ENC Prime). Ставки — свойство СЧЁТА,
// задаются сайтом (админка, портфель), сюда приезжают двумя целыми. Ничего не
// зашито: ноль — значит «без наценки и без комиссии», и счёт без назначенного
// тарифа торгует ровно так, как торговал до тарифов.

/// Верхняя граница ставки тарифа в базисных пунктах (500 bps = 5 %).
///
/// Предохранитель от опечатки в админке, а не коммерческое решение: 5 %
/// наценки или комиссии за сделку уже за пределами любого розничного рынка,
/// и число выше почти наверняка ошибка ввода (лишний ноль).
pub const MAX_FEE_BPS: i64 = 500;

/// Знаменатель базисного пункта.
const BPS: i128 = 10_000;

/// Тариф счёта: наценка на цену исполнения и комиссия от нотионала сделки,
/// оба в базисных пунктах. `Default` — нули: без тарифа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fees {
    /// Наценка на спред: покупка исполняется дороже рыночной цены на
    /// `price · bps / 10000`, продажа — дешевле. На открытии и на закрытии.
    pub markup_bps: i64,
    /// Комиссия от нотионала сделки (`цена исполнения · объём`), списывается
    /// с остатка при КАЖДОМ исполнении: открытие, доливка, закрытие.
    pub commission_bps: i64,
}

impl Fees {
    pub fn is_zero(&self) -> bool {
        self.markup_bps == 0 && self.commission_bps == 0
    }

    /// Обе ставки — целые `0..=MAX_FEE_BPS`.
    pub fn validate(&self) -> Result<(), BrokerError> {
        let ok = |v: i64| (0..=MAX_FEE_BPS).contains(&v);
        if ok(self.markup_bps) && ok(self.commission_bps) {
            Ok(())
        } else {
            Err(BrokerError::InvalidFees)
        }
    }
}

/// Цена исполнения с наценкой.
///
/// `buy` — сторона ИСПОЛНЕНИЯ, а не позиции: открытие лонга и закрытие шорта —
/// покупка (дороже рынка), открытие шорта и закрытие лонга — продажа
/// (дешевле). Сдвиг — `market · bps / 10000`, округление к ближайшему (половина
/// — от нуля): та же политика, что у усреднения входа, — систематическое
/// отбрасывание дарило бы или отнимало деньги в одну сторону.
pub fn apply_markup(buy: bool, market: i64, markup_bps: i64) -> i64 {
    if markup_bps <= 0 {
        return market;
    }
    let off = (market as i128 * markup_bps as i128 + BPS / 2) / BPS;
    let off = off as i64;
    if buy {
        market + off
    } else {
        market - off
    }
}

/// Комиссия за исполнение `qty` по цене `exec`: нотионал · bps / 10000, в
/// наименьших единицах валюты счёта, округление к ближайшему.
pub fn commission_for(
    exec: i64,
    qty: i64,
    pd: u8,
    qd: u8,
    commission_bps: i64,
    minor_per_unit: i128,
) -> Minor {
    if commission_bps <= 0 {
        return 0;
    }
    let notional = value_minor(exec, qty, pd, qd, minor_per_unit);
    (notional * commission_bps as i128 + BPS / 2) / BPS
}

/// Наценка в деньгах: разница цены исполнения и рыночной на объёме.
///
/// Это доход площадки от спреда — та же величина, на которую клиент получил
/// худшую цену. Одна функция на живой путь и на свёртку.
pub fn markup_amount(market: i64, exec: i64, qty: i64, pd: u8, qd: u8, minor: i128) -> Minor {
    value_minor((exec - market).abs(), qty, pd, qd, minor)
}

/// Тариф, под которым позиция открыта, и доход площадки с неё, ещё не
/// отнесённый к закрытой сделке.
///
/// **Снимок, как плечо.** Позиция закрывается по ставкам, действовавшим при
/// её открытии: смена тарифа счёта касается новых позиций, а не тех, на
/// которые человек уже согласился.
///
/// `commission` / `markup` — накопленное по этой позиции (открытие, доливки)
/// и ещё не отнесённое к сделке истории. Закрытие (полное или частичное)
/// забирает накопленное вместе со своим в [`ClosedDeal::fees`] и обнуляет:
/// событие сделки на шине несёт ПОЛНЫЙ доход площадки с неё, а сделка на
/// шину уходит только при закрытии.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PositionFees {
    pub markup_bps: i64,
    pub commission_bps: i64,
    pub commission: Minor,
    pub markup: Minor,
}

impl PositionFees {
    fn snapshot(f: Fees) -> Self {
        Self {
            markup_bps: f.markup_bps,
            commission_bps: f.commission_bps,
            commission: 0,
            markup: 0,
        }
    }
    fn rates(&self) -> Fees {
        Fees {
            markup_bps: self.markup_bps,
            commission_bps: self.commission_bps,
        }
    }
}

/// Доход площадки с закрытой сделки (решение владельца 10.11).
///
/// Нули у сделок без тарифа и у закрытых до появления тарифов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DealFees {
    /// Вся комиссия, отнесённая к этой сделке: открытия (и доливок) и
    /// закрытия. Фактически списанная — после ограничения защитой от минуса.
    pub commission: Minor,
    /// Вся наценка, отнесённая к сделке: на входе и на выходе.
    pub markup: Minor,
    /// Рыночная цена на закрытии — до наценки. `exit` — цена исполнения.
    /// `None` — сделка закрыта до появления поля.
    pub market_exit: Option<i64>,
}

/// Открытая позиция.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub id: u64,
    pub instrument: u32,
    pub side: PosSide,
    pub qty: i64,
    pub entry: i64,
    pub pd: u8,
    pub qd: u8,
    pub margin: Cents,
    pub sl: Option<i64>,
    pub tp: Option<i64>,
    /// Плечо, под которым позиция открыта (TRM-53).
    ///
    /// **Снимок, как категория клиента при открытии счёта.** Маржа под
    /// позицией посчитана по действовавшему плечу, и пересчёт задним числом
    /// либо освободил бы маржу, которую человек не освобождал, либо потребовал
    /// бы сверх того, на что он соглашался.
    ///
    /// Доливка идёт по **этому же** плечу: позиция — единая величина, и маржа
    /// под ней считается одним правилом от открытия до закрытия (решение
    /// штаба). Поэтому запись о доливке своего плеча не несёт — оно уже здесь.
    pub leverage: i128,
    /// Тариф на момент открытия и накопленный доход площадки (10.11).
    pub fees: PositionFees,
}

impl Position {
    /// Нереализованный P&L при марк-цене, в наименьших единицах валюты счёта.
    ///
    /// Точность приходит аргументом, а не хранится на позиции: валюта — свойство
    /// **счёта**, и копия её на каждой позиции была бы вторым источником одной
    /// правды. Позиций у счёта много, валюта одна.
    pub fn unrealized(&self, mark_raw: i64, minor_per_unit: i128) -> Minor {
        let diff = match self.side {
            PosSide::Long => mark_raw - self.entry,
            PosSide::Short => self.entry - mark_raw,
        };
        value_minor(diff, self.qty, self.pd, self.qd, minor_per_unit)
    }
}

/// Отложенный (лимитный) ордер: срабатывает, когда цена достигает `price`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOrder {
    pub id: u64,
    pub instrument: u32,
    pub side: PosSide,
    pub qty: i64,
    pub price: i64,
    pub pd: u8,
    pub qd: u8,
    pub sl: Option<i64>,
    pub tp: Option<i64>,
}

/// Закрытая сделка (история).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedDeal {
    pub instrument: u32,
    pub side: PosSide,
    pub qty: i64,
    pub entry: i64,
    pub exit: i64,
    pub pnl: Cents,
    pub pd: u8,
    pub qd: u8,
    /// Уровень триггера, по которому сделка закрылась: `sl`/`tp`, поставленный
    /// человеком. `None` — закрытия по уровню не было вовсе (рыночное, по
    /// команде, неттинг): там называть нечего (TRM-38).
    ///
    /// Разница между [`Self::exit`] и этим уровнем — **проскальзывание**, и
    /// показать его может только тот, у кого есть обе величины. Прежде наружу
    /// уходила одна `exit`, и восстановить вторую было неоткуда: в журнале она
    /// есть, но журнал наш, а проекции читают другие.
    pub level: Option<i64>,
    /// Закрытие случилось на разорванном ряду цены (Р-073).
    ///
    /// Трёхзначно намеренно: `Some(true)` — разрыв был, `Some(false)` — не
    /// было, **`None` — неизвестно**. `None` стоит у сделок, закрытых до
    /// появления поля: про них мы не знаем, а `false` утверждал бы, что
    /// разрыва не было. Отсутствующее значение ищут, лгущее читают как правду.
    pub across_gap: Option<bool>,
    /// Комиссия и наценка, отнесённые к сделке, и рыночная цена закрытия.
    pub fees: DealFees,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerError {
    InsufficientMargin,
    UnknownPosition,
    UnknownPending,
    /// Вывод больше, чем свободно (за вычетом занятого под маржу).
    InsufficientFunds,
    /// Счёта нет. Отличается от «счёт пуст»: пустой счёт существует.
    UnknownAccount,
    /// Операция только для демо-счёта (TRM-54).
    NotADemoAccount,
    /// На счёте есть открытые позиции или ордера, а операция требует плоского.
    AccountNotFlat,
    /// Стоп-лосс уже по ту сторону цены: позиция закрылась бы на первом же
    /// тике (TRM-57).
    StopLossAlreadyReached,
    /// Тейк-профит уже достигнут: позиция закрылась бы на первом же тике.
    TakeProfitAlreadyReached,
    /// Плечо вне `1..=MAX_LEVERAGE`.
    InvalidLeverage,
    /// Ставка тарифа вне `0..=MAX_FEE_BPS`.
    InvalidFees,
}

/// Условия, на которых счёт исполняет сделку: плечо и тариф для НОВОЙ
/// позиции и правило защиты от минуса для комиссии. Одним значением, чтобы
/// путь открытия не обрастал параметрами по одному.
#[derive(Debug, Clone, Copy)]
struct Terms {
    lev: i128,
    fees: Fees,
    protect: bool,
}

/// Верхняя граница плеча (знаменатель 1:N). Одна на площадку и на счёт: та же,
/// что у `PLATFORM_LEVERAGE`.
pub const MAX_LEVERAGE: i128 = 1000;

/// Сработал бы стоп-лосс `sl` при цене `m` у позиции стороны `side`.
///
/// **Одна формула на две надобности** (TRM-57): по ней прогон триггеров
/// закрывает позицию, и по ней же граница отказывает в уровне, который
/// закрыл бы её сразу. Будь их две, они разошлись бы — и граница принимала бы
/// уровень, срабатывающий на следующем тике, или отказывала в том, что не
/// сработало бы никогда. Та же защита, что у свободной маржи (TRM-41).
pub fn stop_loss_reached(side: PosSide, m: i64, sl: i64) -> bool {
    match side {
        PosSide::Long => m <= sl,
        PosSide::Short => m >= sl,
    }
}

/// Сработал бы тейк-профит `tp` при цене `m`. См. [`stop_loss_reached`].
pub fn take_profit_reached(side: PosSide, m: i64, tp: i64) -> bool {
    match side {
        PosSide::Long => m >= tp,
        PosSide::Short => m <= tp,
    }
}

/// Отказать, если уровень уже по ту сторону цены `at` (TRM-57).
///
/// `at` — цена, относительно которой уровень будет срабатывать: для открытия
/// это **марк-цена** (по ней пойдёт ближайший прогон триггеров), для
/// отложенного ордера — **его собственная цена**, потому что позиция родится
/// именно там, а не там, где рынок сейчас.
fn refuse_levels_already_reached(
    side: PosSide,
    at: i64,
    sl: Option<i64>,
    tp: Option<i64>,
) -> Result<(), BrokerError> {
    if sl.is_some_and(|sl| stop_loss_reached(side, at, sl)) {
        return Err(BrokerError::StopLossAlreadyReached);
    }
    if tp.is_some_and(|tp| take_profit_reached(side, at, tp)) {
        return Err(BrokerError::TakeProfitAlreadyReached);
    }
    Ok(())
}

#[derive(Debug, Default, Clone)]
struct Account {
    /// Неизменная часть счёта: владелец, род, происхождение, категория при
    /// открытии, валюта (TRM-44). Устанавливается при заведении и обычными
    /// операциями не меняется.
    profile: AccountProfile,
    balance: Cents,
    next_id: u64,
    positions: HashMap<u64, Position>,
    pendings: HashMap<u64, PendingOrder>,
    closed: Vec<ClosedDeal>,
    /// Модель ведения позиций **этого счёта** (ADR-025, вариант A). Меняется только
    /// когда счёт «плоский»: пока есть открытые позиции, счёт работает по той модели,
    /// под которой они открыты. Иначе внутри одного счёта смешались бы две модели,
    /// и на вопрос «против какой из встречных неттить» не было бы верного ответа.
    model: PositionModel,
    /// Маржин-колл уже объявлен и ещё не снят (TRM-50).
    ///
    /// **Защёлка, а не вычисляемое условие.** Предупреждение — это переход, а не
    /// состояние: монитор идёт каждые 500 мс, и без защёлки клиент получал бы по
    /// два уведомления в секунду всё время, пока счёт в опасности. Предупреждение,
    /// приходящее дважды в секунду, перестают читать — и тогда оно не
    /// предупреждение, ровно по той же причине, по которой им не является
    /// недошедшее.
    ///
    /// **В слепок НЕ входит и в журнал не пишется:** это не деньги и не факт
    /// ленты, а состояние уведомления. Следствие названо вслух: после
    /// перезапуска счёт, всё ещё находящийся ниже уровня, получит предупреждение
    /// повторно. Это повтор **верного** предупреждения, а не ложное, и он
    /// дешевле столбца в денежной таблице ради состояния переписки.
    margin_called: bool,
    /// Плечо **этого счёта** для новых позиций (Р-446: итог портфеля и
    /// override клиента считает сайт, сюда приезжает одно число).
    ///
    /// `None` — счёту плечо не назначали, действует плечо площадки
    /// (`PLATFORM_LEVERAGE`). Это не «1», а «как у площадки»: счёт, заведённый
    /// до появления поля, обязан торговать так же, как торговал.
    ///
    /// Смена касается только **новых** позиций: открытые держат своё плечо
    /// снимком (`Position::leverage`). Меняется только записью журнала
    /// `LeverageSet` — свёртка обязана сходиться со слепком.
    leverage: Option<i128>,
    /// Тариф **этого счёта** для новых позиций: наценка и комиссия (10.11).
    /// Нули — без тарифа. Как и плечо: назначает сайт, меняется только записью
    /// `FeesSet`, открытые позиции держат свой снимок.
    fees: Fees,
    /// Исходящая очередь событий журнала (TRM-13): что произошло со счётом в текущей
    /// операции. **Не часть состояния** — в слепок не попадает и рестарта не переживает:
    /// её забирает вызывающий сразу после операции и записывает в журнал той же
    /// транзакцией, что и слепок.
    ///
    /// Времени и источника здесь нет: ядро детерминировано (красная линия №2), «кто и
    /// когда» проставляет адаптер при запечатывании записи.
    outbox: Vec<EventKind>,
}

impl Account {
    /// Множитель наименьшей единицы валюты ЭТОГО счёта (TRM-55).
    ///
    /// Один на счёт, берётся из профиля. Все денежные расчёты счёта обязаны
    /// идти через него, а не через константу: «цент» верен только для валют с
    /// двумя знаками.
    fn minor(&self) -> i128 {
        self.profile.currency.minor_per_unit()
    }

    fn used_margin(&self) -> Cents {
        self.positions.values().map(|p| p.margin).sum()
    }

    /// Нереализованный результат по открытым позициям при текущих ценах.
    ///
    /// Инструмент без цены считается по цене входа — то есть «пока не знаем,
    /// движения нет». Выдумывать движение по отсутствующей цене нельзя, а
    /// исключать позицию из расчёта значило бы занизить занятый риск.
    fn open_pnl(&self, marks: &HashMap<u32, i64>) -> Cents {
        self.positions
            .values()
            .map(|p| p.unrealized(*marks.get(&p.instrument).unwrap_or(&p.entry), self.minor()))
            .sum()
    }

    /// Средства счёта с учётом незакрытых позиций.
    fn equity(&self, marks: &HashMap<u32, i64>) -> Cents {
        self.balance + self.open_pnl(marks)
    }

    /// **Единственное определение свободной маржи** (TRM-41).
    ///
    /// Было два, и они расходились: допуск к открытию считал `balance -
    /// used_margin`, а счёт, который видит человек, — `equity - used_margin`.
    /// То есть число на экране не было тем числом, по которому движок решает,
    /// пустить ли сделку. С убыточной открытой позицией экран показывал меньше
    /// свободного, чем движок разрешал; с прибыльной — больше, и человек получал
    /// отказ «не хватает маржи», глядя на экран, где её хватает.
    ///
    /// Верное — через equity: нереализованный убыток уже съел средства, и не
    /// замечать его при допуске значит позволить набирать позиции на деньги,
    /// которых уже нет. Это же число — вход в уровень маржи, по которому
    /// работает принудительное закрытие; два определения сделали бы допуск и
    /// закрытие спорящими об одном счёте.
    fn free_margin(&self, marks: &HashMap<u32, i64>) -> Cents {
        self.equity(marks) - self.used_margin()
    }

    /// Объявить или снять маржин-колл. `Some(уровень)` — предупреждение надо
    /// отправить **сейчас**, то есть уровень только что пересёк порог сверху вниз.
    ///
    /// Снимается не на самом пороге, а **выше полосы возврата**: иначе рынок,
    /// ходящий вокруг уровня, будил бы клиента на каждом переходе.
    ///
    /// **Полосу мы выдумали сами, и это законно — в отличие от самого порога.**
    /// Различие назвал штаб: у порога закрытия есть внешняя правда (регулятор,
    /// практика отрасли), и выдуманное у нас число молча разошлось бы с ней. У
    /// полосы внешней правды нет — это чистое гашение дребезга, наш выбор и
    /// ничей больше. Поэтому её можно выбрать, но она обязана быть настраиваемой
    /// и названной нашим выбором, а не требованием.
    fn note_margin_call(
        &mut self,
        marks: &HashMap<u32, i64>,
        call_pct: i128,
        rearm_band_pct: i128,
    ) -> Option<i128> {
        let Some(level) = self.margin_level_pct(marks) else {
            // Позиций нет — опасности нет, и защёлка обязана сняться: иначе
            // счёт, закрывший всё и открывший заново, не предупредят.
            self.margin_called = false;
            return None;
        };
        if level > call_pct + rearm_band_pct {
            // Вышел за полосу — значит действительно выбрался, а не качнулся.
            self.margin_called = false;
            return None;
        }
        if level > call_pct {
            // В полосе: ещё не предупреждаем заново, но и не предупреждаем
            // повторно. Защёлка как была.
            return None;
        }
        if self.margin_called {
            return None; // уже предупреждён и ещё не выбрался
        }
        self.margin_called = true;
        Some(level)
    }

    /// Уровень маржи в процентах: `equity / занятая маржа · 100`.
    ///
    /// `None` — занятой маржи нет. Это **не** «бесконечно хорошо» и не ноль:
    /// вопрос «насколько счёт близок к принудительному закрытию» счёту без
    /// позиций не задаётся вовсе, и подставлять сюда число значило бы ответить
    /// на незаданный вопрос. Делению на ноль здесь взяться неоткуда именно
    /// потому, что случай назван отдельно.
    fn margin_level_pct(&self, marks: &HashMap<u32, i64>) -> Option<i128> {
        let used = self.used_margin();
        (used > 0).then(|| self.equity(marks) * 100 / used)
    }

    fn log(&mut self, e: EventKind) {
        self.outbox.push(e);
    }

    /// Провести доход площадки с одного исполнения по позиции `id`: наценку
    /// (`MarkupApplied`) и комиссию (`CommissionCharged`).
    ///
    /// **Позиция обязана быть на месте** — проводки относятся к ней, и
    /// свёртка ищет её по тому же id. Поэтому на закрытии это зовётся ДО
    /// снятия позиции, а на открытии — ПОСЛЕ её появления.
    ///
    /// `realized` — результат, который это исполнение вот-вот реализует (на
    /// открытии ноль). Нужен ограничению комиссии защитой от минуса: при
    /// включённой защите комиссия не уводит остаток ниже нуля — списывается
    /// не больше, чем есть после реализации результата, а непогашенная часть
    /// названа в записи (`waived`). Без защиты комиссия списывается целиком,
    /// и минус, если он случился, виден проводкой, а не молча.
    #[allow(clippy::too_many_arguments)]
    fn book_execution(
        &mut self,
        id: u64,
        market: i64,
        exec: i64,
        qty: i64,
        pd: u8,
        qd: u8,
        rates: Fees,
        protect: bool,
        realized: Minor,
    ) {
        let minor = self.minor();
        if rates.markup_bps > 0 {
            let amount = markup_amount(market, exec, qty, pd, qd, minor);
            if let Some(p) = self.positions.get_mut(&id) {
                p.fees.markup += amount;
            }
            self.log(EventKind::MarkupApplied {
                trade: id,
                mkt: market,
                exec,
                amount,
            });
        }
        let due = commission_for(exec, qty, pd, qd, rates.commission_bps, minor);
        if due > 0 {
            let amount = if protect {
                due.min((self.balance + realized).max(0))
            } else {
                due
            };
            self.balance -= amount;
            if let Some(p) = self.positions.get_mut(&id) {
                p.fees.commission += amount;
            }
            self.log(EventKind::CommissionCharged {
                amount,
                trade: id,
                waived: due - amount,
            });
        }
    }

    /// Забрать накопленный по позиции доход площадки в закрываемую сделку.
    fn take_deal_fees(&mut self, id: u64, market: i64) -> DealFees {
        let p = self.positions.get_mut(&id).expect("позиция на месте");
        let f = DealFees {
            commission: p.fees.commission,
            markup: p.fees.markup,
            market_exit: Some(market),
        };
        p.fees.commission = 0;
        p.fees.markup = 0;
        f
    }

    /// `market` — рыночная цена исполнения ДО наценки (ask для покупки, bid
    /// для продажи, наблюдённая цена у отложенного ордера). Наценку
    /// накладывает ядро по тарифу — не вызывающий: иначе цена исполнения
    /// жила бы в двух местах.
    #[allow(clippy::too_many_arguments)]
    fn try_open(
        &mut self,
        t: Terms,
        marks: &HashMap<u32, i64>,
        instrument: u32,
        side: PosSide,
        qty: i64,
        market: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
    ) -> Result<u64, BrokerError> {
        // Уровень, уже достигнутый рынком, — отказ, а не позиция, живущая до
        // первого тика (TRM-57). Сверяемся с МАРК-ценой: по ней пойдёт
        // ближайший прогон триггеров. Цены нет — судить не о чем, и
        // выдумывать отказ не станем.
        if let Some(&m) = marks.get(&instrument) {
            refuse_levels_already_reached(side, m, sl, tp)?;
        }
        match self.model {
            PositionModel::Hedging => {
                self.open_new(t, marks, instrument, side, qty, market, pd, qd, sl, tp)
            }
            PositionModel::Netting => {
                self.open_netting(t, marks, instrument, side, qty, market, pd, qd, sl, tp)
            }
        }
    }

    /// Хеджирование: каждая сделка — новая независимая позиция.
    ///
    /// Позиция получает плечо и тариф счёта снимком. Допуск — маржа ПЛЮС
    /// комиссия открытия: комиссия списывается сразу, и пускать сделку, на
    /// комиссию которой нет свободных средств, значило бы открыть её в долг.
    #[allow(clippy::too_many_arguments)]
    fn open_new(
        &mut self,
        t: Terms,
        marks: &HashMap<u32, i64>,
        instrument: u32,
        side: PosSide,
        qty: i64,
        market: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
    ) -> Result<u64, BrokerError> {
        let lev = t.lev;
        let exec = apply_markup(side == PosSide::Long, market, t.fees.markup_bps);
        let margin = margin_for(exec, qty, pd, qd, lev, self.minor());
        let commission = commission_for(exec, qty, pd, qd, t.fees.commission_bps, self.minor());
        // Свободная маржа считается ОДНИМ способом — через equity (TRM-41).
        if self.free_margin(marks) < margin + commission {
            return Err(BrokerError::InsufficientMargin);
        }
        self.next_id += 1;
        let id = self.next_id;
        self.positions.insert(
            id,
            Position {
                id,
                instrument,
                side,
                qty,
                entry: exec,
                pd,
                qd,
                margin,
                sl,
                tp,
                leverage: lev,
                fees: PositionFees::snapshot(t.fees),
            },
        );
        self.log(EventKind::PositionOpened {
            id,
            instrument,
            side,
            qty,
            entry: exec,
            pd,
            qd,
            sl,
            tp,
            lev,
        });
        self.book_execution(id, market, exec, qty, pd, qd, t.fees, t.protect, 0);
        Ok(id)
    }

    /// Неттинг: на инструмент приходится не больше одной позиции.
    ///
    /// Три случая: позиции нет → обычное открытие; та же сторона → доливка со
    /// средневзвешенным входом; встречная сторона → сворачивание, а остаток (если
    /// встречный объём больше) открывает позицию в новую сторону.
    ///
    /// Возвращает id позиции, которая существует ПОСЛЕ операции. Если встречная
    /// сделка свернула позицию ровно в ноль, возвращается id свёрнутой — вызывающему
    /// нужно чем-то ответить, а новой позиции не появилось.
    ///
    /// Тариф: всё, что касается СУЩЕСТВУЮЩЕЙ позиции (доливка, сворачивание),
    /// идёт по её снимку — как плечо; остаток встречной сделки открывает новую
    /// позицию по тарифу счёта.
    #[allow(clippy::too_many_arguments)]
    fn open_netting(
        &mut self,
        t: Terms,
        marks: &HashMap<u32, i64>,
        instrument: u32,
        side: PosSide,
        qty: i64,
        market: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
    ) -> Result<u64, BrokerError> {
        let existing = self
            .positions
            .values()
            .find(|p| p.instrument == instrument)
            .map(|p| p.id);
        let Some(pos_id) = existing else {
            return self.open_new(t, marks, instrument, side, qty, market, pd, qd, sl, tp);
        };
        let cur = self
            .positions
            .get(&pos_id)
            .expect("позиция только что найдена")
            .clone();
        let rates = cur.fees.rates();
        let exec = apply_markup(side == PosSide::Long, market, rates.markup_bps);

        if cur.side == side {
            // Доливка: объёмы складываются, вход усредняется по объёму.
            let new_qty = cur.qty + qty;
            let new_entry = merged_entry(cur.qty, cur.entry, qty, exec);
            let new_margin = margin_for(new_entry, new_qty, pd, qd, cur.leverage, self.minor());
            // Считаем ДОПОЛНИТЕЛЬНУЮ маржу: часть уже занята этой же позицией.
            let extra = new_margin - cur.margin;
            let commission = commission_for(exec, qty, pd, qd, rates.commission_bps, self.minor());
            // Тем же единственным определением, что и обычное открытие (TRM-41):
            // доливка — это увеличение риска, и считаться она обязана так же.
            let need = extra.max(0) + commission;
            if need > 0 && self.free_margin(marks) < need {
                return Err(BrokerError::InsufficientMargin);
            }
            let p = self.positions.get_mut(&pos_id).expect("позиция на месте");
            p.qty = new_qty;
            p.entry = new_entry;
            p.margin = new_margin;
            // SL/TP новой заявки перекрывают прежние только если заданы: молча
            // терять выставленную защиту нельзя.
            if sl.is_some() {
                p.sl = sl;
            }
            if tp.is_some() {
                p.tp = tp;
            }
            self.log(EventKind::PositionIncreased {
                id: pos_id,
                add_qty: qty,
                add_entry: exec,
                sl,
                tp,
            });
            self.book_execution(pos_id, market, exec, qty, pd, qd, rates, t.protect, 0);
            return Ok(pos_id);
        }

        // Встречная сделка: сворачиваем меньшую из величин.
        let closing = qty.min(cur.qty);
        let pnl = realized_on_close(
            cur.side,
            cur.entry,
            exec,
            closing,
            cur.pd,
            cur.qd,
            self.minor(),
        );
        self.book_execution(
            pos_id, market, exec, closing, cur.pd, cur.qd, rates, t.protect, pnl,
        );
        let fees = self.take_deal_fees(pos_id, market);
        self.balance += pnl;
        self.closed.push(ClosedDeal {
            instrument: cur.instrument,
            side: cur.side,
            qty: closing,
            entry: cur.entry,
            exit: exec,
            pnl,
            pd: cur.pd,
            qd: cur.qd,
            // Неттинг свернул позицию встречной сделкой: уровня тут не было, а
            // разрыв ряда ни при чём — закрыл не триггер, а другая сделка.
            level: None,
            across_gap: Some(false),
            fees,
        });

        let left = cur.qty - closing;
        if left > 0 {
            // Позиция уменьшилась, направление прежнее. Вход не меняется: свёрнута
            // часть, а не перезаключена сделка.
            let minor = self.minor();
            let p = self.positions.get_mut(&pos_id).expect("позиция на месте");
            p.qty = left;
            p.margin = margin_for(p.entry, left, p.pd, p.qd, p.leverage, minor);
            self.log(EventKind::PositionReduced {
                id: pos_id,
                qty: closing,
                exit: exec,
            });
            return Ok(pos_id);
        }

        self.positions.remove(&pos_id);
        self.log(EventKind::PositionClosed {
            id: pos_id,
            exit: exec,
            cause: CloseCause::Netting,
            level: None,
            across_gap: false,
        });
        let remainder = qty - closing;
        if remainder > 0 {
            // Встречный объём больше открытого: остаток открывает позицию в новую
            // сторону. В историю попадает только свёрнутая часть — остаток ещё не
            // закрыт, и место ему не в истории закрытых сделок (ADR-025).
            return self.open_new(
                t, marks, instrument, side, remainder, market, pd, qd, sl, tp,
            );
        }
        Ok(pos_id)
    }

    /// `cause` — не косметика: по журналу надо отличать «человек закрыл» от «сработал
    /// стоп». Иначе при разборе претензии «меня закрыли не по моей команде» ответить
    /// будет нечем, а это ровно тот разговор, ради которого журнал и заводится.
    /// `market` — рыночная цена (у триггеров — наблюдённая, Р-178,
    /// проскальзывание); исполнение — она же с наценкой по снимку тарифа
    /// позиции, и по ней считаются деньги. `level` — уровень, который ставил
    /// человек; у закрытия по команде уровня нет, там `None`.
    ///
    /// Обе нужны вместе: по одной `exec` нельзя отличить «рынок дошёл ровно до
    /// стопа» от «рынок перепрыгнул стоп на разрыве», а это разные разговоры
    /// с клиентом (TRM-31).
    ///
    /// Комиссия закрытия списывается здесь — у любой причины закрытия:
    /// команда, SL/TP, стоп-аут.
    fn close_pos(
        &mut self,
        id: u64,
        market: i64,
        level: Option<i64>,
        across_gap: bool,
        cause: CloseCause,
        protect: bool,
    ) -> Option<Cents> {
        let minor = self.minor();
        let pos = self.positions.get(&id)?.clone();
        let rates = pos.fees.rates();
        // Закрытие лонга — продажа (дешевле рынка), закрытие шорта — покупка.
        let exec = apply_markup(pos.side == PosSide::Short, market, rates.markup_bps);
        let pnl = pos.unrealized(exec, minor);
        self.book_execution(
            id, market, exec, pos.qty, pos.pd, pos.qd, rates, protect, pnl,
        );
        let fees = self.take_deal_fees(id, market);
        self.positions.remove(&id);
        self.balance += pnl;
        self.closed.push(ClosedDeal {
            instrument: pos.instrument,
            side: pos.side,
            qty: pos.qty,
            entry: pos.entry,
            exit: exec,
            pnl,
            pd: pos.pd,
            qd: pos.qd,
            level,
            across_gap: Some(across_gap),
            fees,
        });
        self.log(EventKind::PositionClosed {
            id,
            exit: exec,
            cause,
            level,
            across_gap,
        });
        Some(pnl)
    }

    /// Прогнать триггеры. Возвращает `true`, если состояние счёта изменилось —
    /// по этому признаку gateway понимает, какие счета надо сохранить (ADR-016).
    ///
    /// `across_gap` — этот прогон идёт на первом тике после молчания фида, то
    /// есть ряд цены разорван (Р-073, TRM-19). Поведение триггеров от этого
    /// НЕ меняется: цена, пришедшая после дыры, — настоящая, и прятать её
    /// значило бы не исполнить стоп на реальном движении. Меняется только
    /// запись: закрытие на разрыве должно быть отличимо в журнале, потому что
    /// спорят именно о нём.
    #[allow(clippy::too_many_arguments)]
    fn run_triggers(
        &mut self,
        t: Terms,
        marks: &HashMap<u32, i64>,
        across_gap: bool,
        stop_out_pct: Option<i128>,
        frozen: bool,
    ) -> bool {
        let protect_negative = t.protect;
        // Сработавшие лимитные ордера → открыть позицию.
        let mut fire: Vec<PendingOrder> = Vec::new();
        self.pendings.retain(|_, p| match marks.get(&p.instrument) {
            Some(&m) => {
                let hit = match p.side {
                    PosSide::Long => m <= p.price,  // buy limit — ниже рынка
                    PosSide::Short => m >= p.price, // sell limit — выше рынка
                };
                if hit {
                    fire.push(p.clone());
                    false
                } else {
                    true
                }
            }
            None => true,
        });
        let fired_any = !fire.is_empty();
        for p in fire {
            // Сначала «ордер сработал», потом открытие позиции отдельной записью: это
            // два разных события, и у открывшейся позиции свой id, а не id ордера.
            //
            // Исполняем по НАБЛЮДЁННОЙ цене (Р-178), как и стопы. Для лимитного
            // ордера это всегда НЕ ХУЖЕ его уровня: покупка срабатывает, когда
            // рынок опустился до уровня или ниже, продажа — когда поднялся до
            // уровня или выше. То есть на разрыве клиент получает лучшую цену,
            // и это не щедрость, а то же самое правило: исполняется то, что на
            // рынке, а не то, что мы пообещали нарисовать.
            let exec = marks.get(&p.instrument).copied().unwrap_or(p.price);
            // Счёт заморожен оператором — ордер, поставленный ДО заморозки,
            // позицию не открывает. Проверка на постановке есть, но ордер
            // исполняется позже и без человека: заморозка, проверяемая только
            // на входе, пропускала бы через монитор ровно то, что запрещает.
            // Ордер снимается своей записью с причиной — не молча и не
            // «сработал»: позиции не было, и лента обязана это сказать.
            if frozen {
                self.log(EventKind::PendingRejected {
                    id: p.id,
                    exec: Some(exec),
                    reason: RejectReason::AccountFrozen,
                });
                continue;
            }
            self.log(EventKind::PendingTriggered {
                id: p.id,
                price: p.price,
                exec: Some(exec),
                across_gap,
            });
            // Тариф и плечо — действующие на момент ИСПОЛНЕНИЯ (как с плечом,
            // Р-446): ордер ничего не резервирует, допуск идёт сейчас.
            let _ = self.try_open(
                t,
                marks,
                p.instrument,
                p.side,
                p.qty,
                exec,
                p.pd,
                p.qd,
                p.sl,
                p.tp,
            );
        }

        // SL/TP по открытым позициям. Кортеж несёт обе цены: ИСПОЛНЕНИЯ (по ней
        // считаются деньги) и ПОСТАВЛЕННЫЙ УРОВЕНЬ (её ставил человек). На
        // спокойном рынке они совпадают, на разрыве расходятся — и журналу
        // нужны обе, иначе проскальзывание потом не из чего восстановить.
        let mut to_close: Vec<(u64, i64, i64, CloseCause)> = Vec::new();
        for pos in self.positions.values() {
            if let Some(&m) = marks.get(&pos.instrument) {
                let sl_hit = pos.sl.is_some_and(|sl| stop_loss_reached(pos.side, m, sl));
                let tp_hit = pos
                    .tp
                    .is_some_and(|tp| take_profit_reached(pos.side, m, tp));
                // Исполняем по НАБЛЮДЁННОЙ цене `m` — решение владельца Р-178,
                // проскальзывание. Гарантия уровня означала бы, что разницу на
                // каждом скачке платит брокер: это отдельный продукт, который
                // продают за деньги, а мы отдавали его молча и без предела.
                //
                // Уровень идёт рядом, в запись: человек ставил именно его, и
                // разницу между «поставил» и «исполнилось» он вправе видеть.
                if sl_hit {
                    to_close.push((pos.id, m, pos.sl.unwrap(), CloseCause::StopLoss));
                } else if tp_hit {
                    to_close.push((pos.id, m, pos.tp.unwrap(), CloseCause::TakeProfit));
                }
            }
        }
        let closed_any = !to_close.is_empty();
        for (id, exec, level, cause) in to_close {
            self.close_pos(id, exec, Some(level), across_gap, cause, t.protect);
        }

        // Защита от отрицательного остатка идёт ПОСЛЕ закрытия (TRM-42): она
        // страхует там, где закрытие не успело — на разрыве цена перепрыгивает
        // порог, и позиция закрывается хуже, чем хватает средств.
        //
        // Порядок важен и в обратную сторону: страховка без закрытия означала
        // бы оплачивать каждый провал без ограничения сверху. Поэтому они и
        // делались одним заходом, а не по очереди.
        //
        // Принудительное закрытие по марже — ПОСЛЕ собственных стопов клиента
        // (TRM-41). Порядок не стилистический: если стоп человека сработал на
        // этом же тике, закрыть обязан он, а не мы. Иначе брокер отбирает у
        // клиента исполнение его же распоряжения и ставит себе причину закрытия.
        let stopped_out = self.run_stop_out(marks, across_gap, stop_out_pct, t.protect);
        let compensated = self.cover_negative_balance(protect_negative);

        fired_any || closed_any || stopped_out || compensated
    }

    /// Погасить отрицательный остаток за счёт брокера (TRM-42).
    ///
    /// **Это включённый флаг, а не вшитое поведение.** Сейшелы защиты от
    /// отрицательного баланса не требуют — мы её даём всё равно, и именно
    /// поэтому она обязана быть переключателем: иначе при второй юрисдикции
    /// нечем будет ответить, почему там так же. Поведение, вшитое в код,
    /// невозможно объяснить как выбор.
    ///
    /// **Только когда позиций не осталось.** Пока они открыты, отрицательный
    /// остаток — промежуточное состояние: нереализованная прибыль ещё может его
    /// вернуть, и гасить его значило бы дарить деньги тому, кто выйдет в плюс.
    /// Гасим то, из чего уже нельзя выбраться.
    ///
    /// **Именованной проводкой с суммой, а не записью нуля.** «Стало ноль» не
    /// отвечает на вопрос, сколько мы доплатили, — а это наши деньги, и их надо
    /// уметь сложить за период (решение владельца, ADM-07).
    fn cover_negative_balance(&mut self, enabled: bool) -> bool {
        if !enabled || self.balance >= 0 || !self.positions.is_empty() {
            return false;
        }
        let amount = -self.balance;
        self.balance += amount;
        self.log(EventKind::NegativeBalanceCompensated { amount });
        true
    }

    /// Закрывать, пока уровень маржи не поднимется выше порога.
    ///
    /// **Порог приходит снаружи или закрытия нет вовсе.** Выдуманное на нашей
    /// стороне число молча разошлось бы с тем, по которому считает источник, —
    /// то же правило, по которому порог свежести котировок задаёт MDS, а не
    /// потребитель (КОНТРАКТЫ §5г). Порог — свойство юрисдикции, и его место в
    /// профиле, а не в коде.
    ///
    /// **Закрываем самую убыточную позицию первой.** Арифметически уровень
    /// поднимает любое закрытие: equity от самого факта закрытия не меняется
    /// (нереализованный убыток просто становится реализованным), а занятая маржа
    /// падает. Но выбор не арифметический: убыток течёт из конкретной позиции, и
    /// закрыть сначала её — значит остановить то, что и подвело счёт к порогу.
    /// Закрытие «самой крупной по марже» подняло бы уровень быстрее и оставило
    /// бы течь.
    ///
    /// Позиция без цены **не закрывается**: закрыть по цене входа значило бы
    /// выдумать исполнение. Её маржа продолжает считаться занятой, и если
    /// уровень из-за неё не поднимается, цикл честно останавливается, не
    /// закрыв ничего лишнего.
    fn run_stop_out(
        &mut self,
        marks: &HashMap<u32, i64>,
        across_gap: bool,
        stop_out_pct: Option<i128>,
        protect: bool,
    ) -> bool {
        let Some(threshold) = stop_out_pct else {
            return false;
        };
        let minor = self.minor();
        let mut closed = false;
        loop {
            let used = self.used_margin();
            if used <= 0 {
                break; // позиций нет — закрывать нечего
            }
            // Сравнение без деления: `equity * 100 <= порог * занятая маржа`.
            // Делением мы бы округляли уровень и срабатывали на долю процента
            // раньше или позже порога, в зависимости от величин.
            if self.equity(marks) * 100 > threshold * used {
                break;
            }
            // Самая убыточная ИЗ ТЕХ, у кого есть цена.
            let worst = self
                .positions
                .values()
                .filter_map(|p| {
                    marks
                        .get(&p.instrument)
                        .map(|m| (p.id, *m, p.unrealized(*m, minor)))
                })
                .min_by_key(|(_, _, pnl)| *pnl);
            let Some((id, mark, _)) = worst else {
                break; // цен нет ни по одной — закрывать нечем
            };
            // Уровня человек здесь не ставил: закрытие наше, и называть
            // что-либо «уровнем» было бы приписыванием ему чужого решения.
            self.close_pos(id, mark, None, across_gap, CloseCause::StopOut, protect);
            closed = true;
        }
        closed
    }
}

pub struct Broker {
    start_balance: Cents,
    leverage: i128,
    accounts: HashMap<UserId, Account>,
    /// Уровень маржи в процентах, ниже которого счёт закрывается принудительно
    /// (TRM-49). `None` — **закрытия нет**, и это осознанное состояние, а не
    /// забытая настройка: порог задаёт юрисдикция, и выдумать его здесь значило
    /// бы подменить её решение своим.
    stop_out_pct: Option<i128>,
    /// Гасим ли отрицательный остаток за свой счёт (TRM-42).
    ///
    /// **Флаг, а не вшитое поведение**: Сейшелы такой защиты не требуют, мы её
    /// даём по своему выбору, и этот выбор обязан быть виден и переключаем.
    protect_negative_balance: bool,
    /// Уровень маржи, при достижении которого клиента **предупреждают** (TRM-50).
    /// Выше порога закрытия: предупреждение без запаса не оставляет времени
    /// отреагировать, а закрытие без предупреждения — срабатывание без шанса.
    margin_call_pct: Option<i128>,
    /// Насколько выше уровня предупреждения счёт обязан подняться, чтобы его
    /// предупредили снова. **Наш выбор, не требование** — гашение дребезга.
    margin_call_rearm_band_pct: i128,
}

impl Broker {
    pub fn new(start_balance: Cents, leverage: i128) -> Self {
        Self {
            start_balance,
            leverage: leverage.max(1),
            accounts: HashMap::new(),
            stop_out_pct: None,
            margin_call_pct: None,
            margin_call_rearm_band_pct: 0,
            protect_negative_balance: false,
        }
    }

    /// Задать порог принудительного закрытия (TRM-41).
    ///
    /// Отдельным шагом, а не аргументом конструктора: порог приходит из профиля
    /// юрисдикции, а конструктор зовут и там, где профиля нет (тесты ядра).
    /// Незаданный порог — честное «закрытия нет», и оно видно в `/health`.
    pub fn with_stop_out_pct(mut self, pct: Option<i128>) -> Self {
        self.stop_out_pct = pct;
        self
    }

    pub fn stop_out_pct(&self) -> Option<i128> {
        self.stop_out_pct
    }

    /// Задать уровень предупреждения (TRM-50). Как и порог закрытия — приходит
    /// снаружи или предупреждения нет вовсе.
    pub fn with_margin_call_pct(mut self, pct: Option<i128>) -> Self {
        self.margin_call_pct = pct;
        self
    }

    pub fn margin_call_pct(&self) -> Option<i128> {
        self.margin_call_pct
    }

    /// Полоса возврата для повторного предупреждения (TRM-50). **Наш выбор, а
    /// не требование регулятора:** у ширины полосы внешней правды нет, это
    /// гашение дребезга. Меняется настройкой, без релиза.
    pub fn with_margin_call_rearm_band_pct(mut self, pct: i128) -> Self {
        self.margin_call_rearm_band_pct = pct.max(0);
        self
    }

    pub fn margin_call_rearm_band_pct(&self) -> i128 {
        self.margin_call_rearm_band_pct
    }

    /// Включить защиту от отрицательного остатка (TRM-42).
    pub fn with_negative_balance_protection(mut self, on: bool) -> Self {
        self.protect_negative_balance = on;
        self
    }

    pub fn negative_balance_protection(&self) -> bool {
        self.protect_negative_balance
    }

    /// Счета, которые **сейчас** пересекли уровень предупреждения сверху вниз.
    ///
    /// Вызывать **до** [`Self::check`]: закрытие поднимает уровень, и после него
    /// пересечения уже не видно. Клиент обязан получить предупреждение даже
    /// тогда, когда оба события пришлись на один тик, — иначе мы закрыли счёт и
    /// промолчали о том, что предупреждали бы.
    pub fn margin_calls(&mut self, marks: &HashMap<u32, i64>) -> Vec<(UserId, i128)> {
        let Some(call) = self.margin_call_pct else {
            return Vec::new();
        };
        let band = self.margin_call_rearm_band_pct;
        let mut out = Vec::new();
        for (user, a) in self.accounts.iter_mut() {
            if let Some(level) = a.note_margin_call(marks, call, band) {
                out.push((*user, level));
            }
        }
        out.sort_by_key(|(u, _)| u.0);
        out
    }

    fn acct(&mut self, user: UserId) -> &mut Account {
        let start = self.start_balance;
        // Ленивое заведение достаётся счетам, о профиле которых никто не сказал:
        // это путь проверок ядра. В бою счёт заводит `open_account` с настоящим
        // профилем, и до сюда дело не доходит (TRM-46).
        let profile = AccountProfile::legacy_demo(user);
        self.accounts.entry(user).or_insert_with(|| {
            // Заведение счёта — тоже изменение состояния, и стартовый баланс обязан
            // иметь происхождение. Без этой записи первые деньги на счёте взялись бы
            // ниоткуда, и журнал начинался бы с необъяснимой суммы.
            let mut a = Account {
                balance: start,
                profile,
                ..Default::default()
            };
            a.log(EventKind::AccountOpened {
                balance: start,
                profile,
            });
            a
        })
    }

    /// Есть ли счёт в памяти. Отличается от «баланс равен стартовому»: до первого
    /// обращения счёта **нет**, и `balance()` отдаёт стартовую сумму как ответ на
    /// вопрос «сколько было бы», а не как состояние существующего счёта.
    pub fn has_account(&self, user: UserId) -> bool {
        self.accounts.contains_key(&user)
    }

    /// Завести счёт **явно**, отдельно от любой операции (TRM-46).
    ///
    /// Возвращает `true`, если счёт только что появился, — тогда в очереди лежит
    /// [`EventKind::AccountOpened`], и забрать его обязан вызывающий.
    ///
    /// Зачем отдельная ручка, когда счёт и так заводится лениво при первой
    /// операции. Затем, что ленивое заведение привязывает рождение счёта к ИСХОДУ
    /// чужой операции: очередь сливается и на отказе — справедливо для событий
    /// операции и неверно для рождения, потому что счёт при отказе **остаётся**.
    /// Запись о нём уезжала в никуда, второй раз её никто не напишет, и первые
    /// деньги на счёте оставались без происхождения (найдено 24.10 живым
    /// прогоном). Разделять надо там, где разные факты, а не ловить их на выходе.
    pub fn open_account(&mut self, user: UserId) -> bool {
        self.open_account_with(user, AccountProfile::legacy_demo(user), self.start_balance)
    }

    /// Завести счёт с **названным профилем** и стартовым балансом по его роду
    /// (TRM-44).
    ///
    /// Стартовый баланс определяет род, а не вызывающий: **реальный счёт
    /// открывается нулём**. Ненулевой начальный баланс на реальном счёте — это
    /// деньги, взявшиеся ниоткуда, и решать это у места вызова нельзя, иначе
    /// однажды решат иначе.
    ///
    /// `demo_start` — сумма, с которой открывается **демо**-счёт, и она
    /// передаётся, а не читается из поля брокера.
    ///
    /// **Это та же развязка, что с плечом (TRM-53).** Сумма — настройка
    /// площадки, её меняет владелец в админке; будь она полем брокера, правка
    /// задним числом меняла бы то, с чего открывались прошлые счета. Как
    /// вход она попадает в запись `AccountOpened { balance }` и остаётся в
    /// истории навсегда: свёртка берёт остаток оттуда, а не из сегодняшней
    /// настройки.
    ///
    /// **Ноль законен.** Демо-счёт на ноль — это решение площадки, а не
    /// отсутствие значения; отличать «ноль» от «не сказали» обязан тот, кто
    /// читает источник, и до сюда «не сказали» не доходит.
    pub fn open_account_with(
        &mut self,
        user: UserId,
        profile: AccountProfile,
        demo_start: Cents,
    ) -> bool {
        if self.accounts.contains_key(&user) {
            return false;
        }
        let start = start_balance_for(profile.kind, demo_start);
        let mut a = Account {
            balance: start,
            profile,
            ..Default::default()
        };
        a.log(EventKind::AccountOpened {
            balance: start,
            profile,
        });
        self.accounts.insert(user, a);
        true
    }

    /// Убрать счёт из памяти целиком — **только** для отката неудавшегося
    /// заведения (TRM-46).
    ///
    /// Нужна ровно потому, что заведение стало отдельным шагом: если его запись в
    /// хранилище не прошла, счёт обязан исчезнуть и из памяти. Иначе он остаётся
    /// заведённым без журнала, а повторная попытка уже не напишет
    /// [`EventKind::AccountOpened`] — счёт существует, — и мы получим ровно тот
    /// дефект, от которого ушли.
    ///
    /// Возвращает `true`, если счёт был. Данные при этом теряются, поэтому
    /// вызывать её можно **только** сразу после неудачного [`Self::open_account`],
    /// пока на счёте заведомо ничего нет.
    pub fn forget(&mut self, user: UserId) -> bool {
        self.accounts.remove(&user).is_some()
    }

    /// Забрать события, накопленные операциями с этим счётом, и очистить очередь.
    ///
    /// Забирать обязан вызывающий — **и на отказе тоже**: события неудавшейся операции
    /// не должны уехать в журнал вместе со следующей.
    pub fn take_events(&mut self, user: UserId) -> Vec<EventKind> {
        self.accounts
            .get_mut(&user)
            .map(|a| std::mem::take(&mut a.outbox))
            .unwrap_or_default()
    }

    /// Есть ли неразобранные события по счёту (для проверок вызывающего).
    pub fn pending_events(&self, user: UserId) -> usize {
        self.accounts
            .get(&user)
            .map(|a| a.outbox.len())
            .unwrap_or(0)
    }

    // ---- Чтение -----------------------------------------------------------

    /// Остаток счёта. `None` — **счёта нет**, и это не ноль (TRM-58).
    ///
    /// Прежде здесь стояло `unwrap_or(self.start_balance)`: несуществующему
    /// счёту приписывалась настройка площадки. Число выглядело остатком
    /// конкретного человека, и ручка `/account` отдавала его наружу.
    /// Совпадало оно случайно — все счета сегодня демо и заводятся с той же
    /// суммы; у реального счёта стартовый остаток ноль, и та же подстановка
    /// показала бы ему чужие десять тысяч.
    ///
    /// Это ровно BUG-011, этажом ниже: «ноль — правдоподобный баланс, по нему
    /// принимают решение». Правдоподобна и настройка площадки.
    pub fn balance(&self, user: UserId) -> Option<Cents> {
        self.accounts.get(&user).map(|a| a.balance)
    }
    pub fn used_margin(&self, user: UserId) -> Cents {
        self.accounts
            .get(&user)
            .map(|a| a.used_margin())
            .unwrap_or(0)
    }
    /// Свободная маржа — через equity, **одним определением на весь счёт**
    /// (TRM-41). Требует цены: нереализованный результат — часть средств.
    /// `None` — счёта нет (TRM-58). Прежде здесь тоже подставлялась стартовая
    /// настройка площадки, и это было опаснее подставленного остатка:
    /// свободная маржа — то число, по которому решают, пускать ли сделку.
    pub fn free_margin(&self, user: UserId, marks: &HashMap<u32, i64>) -> Option<Cents> {
        self.accounts.get(&user).map(|a| a.free_margin(marks))
    }

    /// Уровень маржи в процентах: `equity / занятая маржа`.
    ///
    /// `None` — занятой маржи нет, и уровень **не определён**. Не «бесконечность»
    /// и не ноль: счёт без позиций нельзя ни закрыть принудительно, ни признать
    /// здоровым по этому показателю — вопрос к нему не задаётся.
    pub fn margin_level_pct(&self, user: UserId, marks: &HashMap<u32, i64>) -> Option<i128> {
        self.accounts
            .get(&user)
            .and_then(|a| a.margin_level_pct(marks))
    }
    pub fn positions(&self, user: UserId) -> Vec<Position> {
        self.accounts
            .get(&user)
            .map(|a| a.positions.values().cloned().collect())
            .unwrap_or_default()
    }
    pub fn pendings(&self, user: UserId) -> Vec<PendingOrder> {
        self.accounts
            .get(&user)
            .map(|a| a.pendings.values().cloned().collect())
            .unwrap_or_default()
    }
    pub fn closed_deals(&self, user: UserId) -> Vec<ClosedDeal> {
        self.accounts
            .get(&user)
            .map(|a| a.closed.clone())
            .unwrap_or_default()
    }
    pub fn open_pnl(&self, user: UserId, marks: &HashMap<u32, i64>) -> Cents {
        self.accounts
            .get(&user)
            .map(|a| {
                a.positions
                    .values()
                    .map(|p| {
                        p.unrealized(
                            *marks.get(&p.instrument).unwrap_or(&p.entry),
                            a.profile.currency.minor_per_unit(),
                        )
                    })
                    .sum()
            })
            .unwrap_or(0)
    }
    /// Средства счёта. `None` — счёта нет (TRM-58): складывать нечего, и ноль
    /// здесь был бы таким же правдоподобным ответом на незаданный вопрос, как
    /// подставленный остаток.
    pub fn equity(&self, user: UserId, marks: &HashMap<u32, i64>) -> Option<Cents> {
        self.balance(user).map(|b| b + self.open_pnl(user, marks))
    }

    // ---- Операции ---------------------------------------------------------

    /// Открыть позицию по рынку (с опциональными SL/TP).
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &mut self,
        user: UserId,
        marks: &HashMap<u32, i64>,
        instrument: u32,
        side: PosSide,
        qty: i64,
        entry: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
    ) -> Result<u64, BrokerError> {
        // Плечо СЧЁТА, а если не назначено — площадки. Оно ложится снимком в
        // позицию и дальше от счёта не зависит (TRM-53).
        // Тариф счёта — так же снимком (10.11).
        let platform = self.leverage;
        let protect = self.protect_negative_balance;
        let a = self.acct(user);
        let t = Terms {
            lev: a.leverage.unwrap_or(platform),
            fees: a.fees,
            protect,
        };
        a.try_open(t, marks, instrument, side, qty, entry, pd, qd, sl, tp)
    }

    /// Закрыть позицию по марк-цене. Возвращает реализованный P&L.
    ///
    /// `mark` — рыночная цена; исполнение — с наценкой по снимку тарифа
    /// позиции, комиссия закрытия списывается отдельной проводкой.
    pub fn close(&mut self, user: UserId, id: u64, mark: i64) -> Result<Cents, BrokerError> {
        // Закрытие по команде исполняется по той же цене, которую видели:
        // расхождения нет, и `level` здесь молчит — не потому что неизвестно,
        // а потому что назвать нечего.
        let protect = self.protect_negative_balance;
        self.acct(user)
            .close_pos(id, mark, None, false, CloseCause::Command, protect)
            .ok_or(BrokerError::UnknownPosition)
    }

    /// Пополнить счёт (демо: бумажные деньги). Возвращает новый баланс.
    ///
    /// Ручное начисление центра управления (Р-049) — это она же: сумма та же, разница
    /// в `Origin` записи, который проставляет адаптер. Отдельного «исправить остаток»
    /// не будет: остаток не перезаписывается, он выводится из проводок.
    pub fn deposit(&mut self, user: UserId, amount: Cents) -> Cents {
        let a = self.acct(user);
        a.balance += amount;
        a.log(EventKind::Deposited { amount });
        a.balance
    }

    /// Вывести со счёта. Нельзя вывести больше, чем свободно (баланс минус занятое
    /// под маржу открытых позиций). Возвращает новый баланс.
    pub fn withdraw(
        &mut self,
        user: UserId,
        amount: Cents,
        marks: &HashMap<u32, i64>,
    ) -> Result<Cents, BrokerError> {
        // Счёта нет — свободных средств нет, и вывод отказан (TRM-58).
        //
        // Прежде сравнение шло со стартовой настройкой площадки, и вывод с
        // НЕСУЩЕСТВУЮЩЕГО счёта проходил: `acct()` заводил его тут же и
        // выдавал стартовую сумму. Наружу эта ручка не выведена, так что
        // следов в проде нет, но поведение было именно такое.
        let Some(free) = self.free_margin(user, marks) else {
            return Err(BrokerError::InsufficientFunds);
        };
        if amount > free {
            return Err(BrokerError::InsufficientFunds);
        }
        let a = self.acct(user);
        a.balance -= amount;
        a.log(EventKind::Withdrawn { amount });
        Ok(a.balance)
    }

    /// Разместить отложенный (лимитный) ордер. Возвращает его id.
    ///
    /// Уровни сверяются с **ценой самого ордера**, а не с рынком (TRM-57):
    /// позиция родится там, где ордер исполнится. Лимит на покупку стоит ниже
    /// рынка по замыслу, и сверка с текущей ценой отвергала бы здоровые
    /// ордера, пропуская как раз больные.
    #[allow(clippy::too_many_arguments)]
    pub fn place_pending(
        &mut self,
        user: UserId,
        instrument: u32,
        side: PosSide,
        qty: i64,
        price: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
    ) -> Result<u64, BrokerError> {
        refuse_levels_already_reached(side, price, sl, tp)?;
        let a = self.acct(user);
        a.next_id += 1;
        let id = a.next_id;
        a.pendings.insert(
            id,
            PendingOrder {
                id,
                instrument,
                side,
                qty,
                price,
                pd,
                qd,
                sl,
                tp,
            },
        );
        a.log(EventKind::PendingPlaced {
            id,
            instrument,
            side,
            qty,
            price,
            pd,
            qd,
            sl,
            tp,
        });
        Ok(id)
    }

    pub fn cancel_pending(&mut self, user: UserId, id: u64) -> Result<(), BrokerError> {
        let a = self.acct(user);
        if a.pendings.remove(&id).is_some() {
            a.log(EventKind::PendingCancelled { id });
            Ok(())
        } else {
            Err(BrokerError::UnknownPending)
        }
    }

    /// Прогнать триггеры лимитных ордеров и SL/TP по всем счетам при текущих марк-ценах.
    ///
    /// Возвращает **id счетов, состояние которых изменилось**. Обычный случай — цена
    /// движется, но ничего не сработало → пустой вектор, сохранять нечего. Без этого
    /// пришлось бы писать в БД все счета на каждом тике монитора (ADR-016).
    /// `across_gap` — первый прогон после молчания фида (Р-073). Передаёт его
    /// вызывающий: о свежести цен знает тот, кто их принимает, а не брокер.
    pub fn check(&mut self, marks: &HashMap<u32, i64>, across_gap: bool) -> Vec<UserId> {
        self.check_with_frozen(marks, across_gap, &HashSet::new())
    }

    /// [`Self::check`] с перечнем счетов, торговля по которым заморожена.
    ///
    /// Заморозку держит шлюз (свёртка журнала, `TradingFreezeSet`), а не
    /// брокер, поэтому она приходит аргументом. У замороженного счёта
    /// сработавший отложенный ордер **снимается** (`PendingRejected`), а не
    /// исполняется; стопы, тейки и стоп-аут работают как обычно — они
    /// закрывают риск, а не открывают его.
    ///
    /// Плечо исполнения отложенного ордера — **действующее плечо счёта на
    /// момент исполнения**, а не постановки. Ордер маржу не резервирует, допуск
    /// по марже идёт при исполнении, и плечо — вход того же расчёта. Снимок
    /// плеча в ордере пропускал бы через монитор решение, которое оператор уже
    /// отменил (снизил плечо по риску), — та же дыра, что с заморозкой, только
    /// в цифрах. Позиция, родившаяся из ордера, дальше держит своё плечо
    /// снимком, как любая другая.
    pub fn check_with_frozen(
        &mut self,
        marks: &HashMap<u32, i64>,
        across_gap: bool,
        frozen: &HashSet<UserId>,
    ) -> Vec<UserId> {
        let platform = self.leverage;
        let stop_out = self.stop_out_pct;
        let protect = self.protect_negative_balance;
        let mut changed = Vec::new();
        for (user, a) in self.accounts.iter_mut() {
            let t = Terms {
                lev: a.leverage.unwrap_or(platform),
                fees: a.fees,
                protect,
            };
            let is_frozen = frozen.contains(user);
            if a.run_triggers(t, marks, across_gap, stop_out, is_frozen) {
                changed.push(*user);
            }
        }
        changed
    }

    // ---- Персистентность (ADR-016) ----------------------------------------
    //
    // Брокер остаётся in-memory и синхронным: он не знает ни про БД, ни про I/O
    // (принцип №6). Наружу отдаётся слепок счёта, а сохраняет и загружает его
    // адаптер в gateway через крейт `persistence`.

    /// Слепок счёта для сохранения. Позиции и ордера переписываются целиком —
    /// их единицы-десятки; история закрытых сделок только дописывается.
    pub fn snapshot(&self, user: UserId) -> AccountSnapshot {
        match self.accounts.get(&user) {
            Some(a) => AccountSnapshot {
                profile: a.profile,
                balance: a.balance,
                next_id: a.next_id,
                positions: a.positions.values().cloned().collect(),
                pendings: a.pendings.values().cloned().collect(),
                closed: a.closed.clone(),
                model: a.model,
                leverage: a.leverage,
                fees: a.fees,
            },
            None => AccountSnapshot {
                profile: AccountProfile::legacy_demo(user),
                balance: self.start_balance,
                next_id: 0,
                positions: vec![],
                pendings: vec![],
                closed: vec![],
                model: PositionModel::default(),
                leverage: None,
                fees: Fees::default(),
            },
        }
    }

    /// Восстановить счёт из хранилища на старте. Полностью заменяет состояние счёта.
    ///
    /// Очередь событий при этом **обнуляется**, и это не оплошность: `restore` служит
    /// ещё и откатом неудавшейся операции, а события отката в журнал попасть не должны —
    /// журнал пишется только о том, что состоялось.
    pub fn restore(&mut self, user: UserId, snap: AccountSnapshot) {
        let a = Account {
            profile: snap.profile,
            balance: snap.balance,
            next_id: snap.next_id,
            positions: snap.positions.into_iter().map(|p| (p.id, p)).collect(),
            pendings: snap.pendings.into_iter().map(|p| (p.id, p)).collect(),
            closed: snap.closed,
            model: snap.model,
            leverage: snap.leverage,
            fees: snap.fees,
            // Защёлка предупреждения состояние счёта не описывает и в слепке её
            // нет: после подъёма счёт, всё ещё лежащий ниже уровня, будет
            // предупреждён заново. Повтор верного предупреждения дешевле
            // столбца в денежной таблице ради состояния переписки (TRM-50).
            margin_called: false,
            outbox: Vec::new(),
        };
        self.accounts.insert(user, a);
    }

    /// Сбросить демо-счёт к стартовому остатку (TRM-54).
    ///
    /// **Владелец денежного числа — терминал.** Кабинет отражает, а не пишет:
    /// до этой ручки он правил свою копию у себя, и следующее же событие о
    /// балансе возвращало прежнюю сумму — сброс отменялся сам.
    ///
    /// Отказывает в двух случаях, и оба названы:
    ///
    /// - **счёт не демо.** Сбрасывать настоящие деньги к «стартовой сумме»
    ///   нечему: её там нет, реальный счёт открывается нулём;
    /// - **счёт не плоский.** Сброс при открытых позициях оставил бы остаток
    ///   от одной жизни счёта и позиции от другой. Закрывать их самим значило
    ///   бы провести денежную операцию, которой никто не просил, — а закрытие
    ///   позиции это сделка с ценой и результатом.
    ///
    /// Записывается **движением со знаком**, а не стартовой суммой: по журналу
    /// должно восстанавливаться, сколько списали или добавили.
    pub fn reset_demo(&mut self, user: UserId) -> Result<Cents, BrokerError> {
        let start = self.start_balance;
        let a = self
            .accounts
            .get_mut(&user)
            .ok_or(BrokerError::UnknownAccount)?;
        if a.profile.kind != AccountKind::Demo {
            return Err(BrokerError::NotADemoAccount);
        }
        if !a.positions.is_empty() || !a.pendings.is_empty() {
            return Err(BrokerError::AccountNotFlat);
        }
        let target = start_balance_for(a.profile.kind, start);
        let adjustment = target - a.balance;
        if adjustment == 0 {
            return Ok(a.balance); // уже на стартовом: проводка без движения не нужна
        }
        a.balance += adjustment;
        a.log(EventKind::DemoAccountReset { adjustment });
        Ok(a.balance)
    }

    /// Применить модель тенанта к счёту (ADR-025, вариант A): смена принимается
    /// только когда счёт «плоский». Возвращает `true`, если модель принята.
    ///
    /// Пока позиции есть, счёт продолжает работать по прежней модели — и это не
    /// отказ, а отложенный переход: каждый счёт мигрирует в свой ближайший момент
    /// без позиций.
    pub fn adopt_model(&mut self, user: UserId, model: PositionModel) -> bool {
        let a = self.acct(user);
        if a.model == model {
            return true;
        }
        if a.positions.is_empty() {
            a.model = model;
            a.log(EventKind::ModelAdopted { model });
            true
        } else {
            false
        }
    }

    /// Модель, по которой сейчас работает счёт.
    pub fn model(&self, user: UserId) -> PositionModel {
        self.accounts
            .get(&user)
            .map(|a| a.model)
            .unwrap_or_default()
    }

    /// Плечо площадки (`PLATFORM_LEVERAGE`) — умолчание для счёта, которому
    /// своё плечо не назначали.
    pub fn leverage(&self) -> i128 {
        self.leverage
    }

    /// Плечо, с которым у этого счёта откроется **следующая** позиция: своё
    /// плечо счёта, а если его не назначали — плечо площадки. Для счёта,
    /// которого ещё нет, — плечо площадки: с ним он и откроется.
    pub fn account_leverage(&self, user: UserId) -> i128 {
        self.assigned_leverage(user).unwrap_or(self.leverage)
    }

    /// Назначено ли счёту своё плечо (`None` — действует плечо площадки).
    pub fn assigned_leverage(&self, user: UserId) -> Option<i128> {
        self.accounts.get(&user).and_then(|a| a.leverage)
    }

    /// Назначить плечо счёту (Р-446). Касается только **новых** позиций:
    /// открытые держат своё плечо снимком, и маржа под ними не меняется.
    ///
    /// `Ok(true)` — плечо сменилось, в очереди лежит `LeverageSet`;
    /// `Ok(false)` — оно уже такое, записи нет (повтор не шумит в журнал).
    /// Счёта нет — отказ, а не заведение: счёт заводит шлюз своей записью.
    pub fn set_account_leverage(&mut self, user: UserId, lev: i128) -> Result<bool, BrokerError> {
        if !(1..=MAX_LEVERAGE).contains(&lev) {
            return Err(BrokerError::InvalidLeverage);
        }
        let a = self
            .accounts
            .get_mut(&user)
            .ok_or(BrokerError::UnknownAccount)?;
        if a.leverage == Some(lev) {
            return Ok(false);
        }
        a.leverage = Some(lev);
        a.log(EventKind::LeverageSet { lev });
        Ok(true)
    }

    /// Тариф счёта для **новых** позиций (нули — без тарифа). Для счёта,
    /// которого нет, — нули: с ними он и откроется.
    pub fn account_fees(&self, user: UserId) -> Fees {
        self.accounts.get(&user).map(|a| a.fees).unwrap_or_default()
    }

    /// Назначить счёту тариф (10.11): наценку и комиссию в bps. Касается
    /// только **новых** позиций — открытые закрываются по своему снимку.
    ///
    /// `Ok(true)` — тариф сменился, в очереди `FeesSet`; `Ok(false)` — уже
    /// такой, записи нет. Счёта нет — отказ, а не заведение.
    pub fn set_account_fees(&mut self, user: UserId, fees: Fees) -> Result<bool, BrokerError> {
        fees.validate()?;
        let a = self
            .accounts
            .get_mut(&user)
            .ok_or(BrokerError::UnknownAccount)?;
        if a.fees == fees {
            return Ok(false);
        }
        a.fees = fees;
        a.log(EventKind::FeesSet {
            markup_bps: fees.markup_bps,
            commission_bps: fees.commission_bps,
        });
        Ok(true)
    }

    /// Сменить плечо площадки (TRM-53 шаг 2).
    ///
    /// **Настройка, а не выпуск.** Плечо — свойство юрисдикции, и владелец
    /// меняет его без сборки образа. Менять его безопасно ровно потому, что
    /// маржа каждой открытой позиции считается по плечу, записанному В НЕЙ:
    /// новое значение достаётся только позициям, открытым после него.
    ///
    /// Ноль и отрицательное отвергаются молча-но-видимо — значение не
    /// применяется, и функция отдаёт `false`: плечо делит нотионал, нулевое
    /// уронило бы расчёт, а отрицательное дало бы отрицательную маржу. Тот же
    /// отказ, что у `lev_added_later` в журнале, и по той же причине.
    pub fn set_leverage(&mut self, lev: i128) -> bool {
        if lev <= 0 {
            return false;
        }
        self.leverage = lev;
        true
    }

    /// Стартовый баланс счёта — нужен журналу: с него начинается свёртка счёта,
    /// у которого ещё нет ни одной записи.
    pub fn start_balance(&self) -> Cents {
        self.start_balance
    }

    /// Все счета, о которых знает брокер (для сохранения/обхода).
    pub fn users(&self) -> Vec<UserId> {
        self.accounts.keys().copied().collect()
    }
}

/// Слепок состояния одного счёта — единица обмена с хранилищем (ADR-016).
#[derive(Debug, Clone, Default)]
pub struct AccountSnapshot {
    /// Владелец, род, происхождение, категория при открытии, валюта (TRM-44).
    /// У счетов, поднятых из хранилища без этих столбцов, —
    /// [`AccountProfile::legacy_demo`]: демо, розничный, доллар, регистрация.
    /// Не умолчание «на всякий случай», а факт их истории.
    pub profile: AccountProfile,
    pub balance: Cents,
    pub next_id: u64,
    pub positions: Vec<Position>,
    pub pendings: Vec<PendingOrder>,
    pub closed: Vec<ClosedDeal>,
    /// Модель ведения позиций счёта (ADR-025). Счета, заведённые до 2026-09-04,
    /// поднимаются как `Hedging` — другой модели в коде не существовало.
    pub model: PositionModel,
    /// Плечо счёта для новых позиций (Р-446). `None` — не назначали, действует
    /// плечо площадки.
    pub leverage: Option<i128>,
    /// Тариф счёта для новых позиций (10.11). Нули — без тарифа.
    pub fees: Fees,
}

// ======================= Инварианты денег (красная линия №5) =================
//
// Проверки — **чистые функции над слепком счёта**: от хранилища не зависят и
// работают одинаково с БД и без неё. Прежняя проверка жила в `PgStore` и на
// `NoopStore` была пустой — а на `NoopStore` идут все тесты, то есть набор ни разу
// не выполнял то, ради чего проверка написана (TRM-06). Проверка, отключённая в
// том режиме, где её проверяют, не существует.

/// Нарушение денежного инварианта. Текст — для лога и трекера, не для клиента:
/// наружу отдаётся код, а не внутренности счёта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation(pub String);

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Стартовый баланс по роду счёта (TRM-44, решение владельца).
///
/// **Реальный счёт открывается нулём.** Демо — настроенной суммой. Правило
/// живёт здесь, а не у места вызова: иначе однажды его применят иначе, и на
/// реальном счёте появятся деньги, взявшиеся ниоткуда.
pub fn start_balance_for(kind: AccountKind, demo_start: Cents) -> Cents {
    match kind {
        AccountKind::Demo => demo_start,
        AccountKind::Real => 0,
    }
}

/// Проверить слепок счёта. Пустой вектор — всё сходится.
///
/// Чего здесь сознательно НЕТ: проверки `balance < 0`. Она была единственной и
/// фатальной, а с плечом > 1 отрицательный баланс законен — то есть проверка,
/// написанная под допущение `leverage = 1`, не давала бы сервису стартовать на
/// корректном состоянии в день включения плеча (TRM-06). Неверная проверка,
/// которая сегодня проходит, опаснее отсутствующей: она сработает позже и на
/// правильных данных.
pub fn check_snapshot(snap: &AccountSnapshot) -> Vec<Violation> {
    let mut out = Vec::new();
    let v = |out: &mut Vec<Violation>, msg: String| out.push(Violation(msg));

    // 1. Маржа каждой позиции обязана сходиться с пересчитанной из её же
    //    параметров: нотионал / плечо.
    //
    //    ⚠️ Первая версия этой проверки сравнивала сумму маржи позиций с суммой
    //    маржи позиций — то есть значение с самим собой — и не могла сработать
    //    никогда. Нашлось мутацией: снятие проверки не покрасило ни одного теста.
    //    Сравнивать надо СОХРАНЁННОЕ с ПЕРЕСЧИТАННЫМ, иначе проверяется тождество.
    //    Это единственный способ поймать маржу, приехавшую из хранилища кривой
    //    или посчитанную при другом плече.
    // Пересчёт идёт в валюте ЭТОГО счёта: та же проверка с зашитым центом
    // объявила бы здоровую маржу иенового счёта неверной в сто раз.
    let minor = snap.profile.currency.minor_per_unit();
    for p in &snap.positions {
        // Плечо берётся У ПОЗИЦИИ, а не у площадки (TRM-53 шаг 2).
        //
        // **Проверка обязана сверять счёт с его собственным прошлым.** Пока
        // здесь стояло сегодняшнее плечо, смена настройки объявляла неверной
        // маржу каждой открытой позиции разом — счета уезжали в карантин
        // целыми пачками, хотя испорчено не было ничего. Расхождение значило
        // «настройку поменяли», а не «данные кривые», и как сигнал о порче
        // проверка переставала работать именно тогда, когда была нужна.
        let lev = p.leverage.max(1);
        let expected = value_minor(p.entry, p.qty, p.pd, p.qd, minor) / lev;
        if p.margin != expected {
            v(
                &mut out,
                format!(
                    "позиция {}: маржа {} != пересчитанной {expected} (плечо {lev})",
                    p.id, p.margin
                ),
            );
        }
    }

    // 2. Маржа позиции не бывает отрицательной, объём и цена входа — строго больше нуля.
    //    Позиция с нулевым объёмом даёт нулевой P&L при ненулевой занятой марже;
    //    позиция с нулевой ценой входа — бесконечный процентный результат.
    for p in &snap.positions {
        if p.qty <= 0 {
            v(
                &mut out,
                format!("позиция {}: объём {} не больше нуля", p.id, p.qty),
            );
        }
        if p.entry <= 0 {
            v(
                &mut out,
                format!("позиция {}: цена входа {} не больше нуля", p.id, p.entry),
            );
        }
        if p.margin < 0 {
            v(
                &mut out,
                format!("позиция {}: отрицательная маржа {}", p.id, p.margin),
            );
        }
    }

    // 2а. Тариф (10.11): ставки в пределах, накопленный доход не отрицательный.
    //     Ставка вне пределов из хранилища — испорченная строка: по ней
    //     пошли бы деньги каждой следующей сделки.
    if snap.fees.validate().is_err() {
        v(
            &mut out,
            format!("тариф счёта вне 0..={MAX_FEE_BPS} bps: {:?}", snap.fees),
        );
    }
    for p in &snap.positions {
        if p.fees.rates().validate().is_err() {
            v(
                &mut out,
                format!("позиция {}: тариф вне 0..={MAX_FEE_BPS} bps", p.id),
            );
        }
        if p.fees.commission < 0 || p.fees.markup < 0 {
            v(
                &mut out,
                format!("позиция {}: отрицательный накопленный доход площадки", p.id),
            );
        }
    }

    // 3. То же для отложенных ордеров: они станут позициями по этим же числам.
    for o in &snap.pendings {
        if o.qty <= 0 {
            v(
                &mut out,
                format!("ордер {}: объём {} не больше нуля", o.id, o.qty),
            );
        }
        if o.price <= 0 {
            v(
                &mut out,
                format!("ордер {}: цена {} не больше нуля", o.id, o.price),
            );
        }
    }

    // 4. Идентификаторы уникальны и меньше следующего: иначе новая позиция затрёт
    //    существующую, и одна из них исчезнет вместе с её маржой.
    let mut ids: Vec<u64> = snap
        .positions
        .iter()
        .map(|p| p.id)
        .chain(snap.pendings.iter().map(|o| o.id))
        .collect();
    ids.sort_unstable();
    let total = ids.len();
    ids.dedup();
    if ids.len() != total {
        v(
            &mut out,
            "идентификаторы позиций и ордеров не уникальны".to_string(),
        );
    }
    // ⚠️ `next_id` вопреки имени хранит ПОСЛЕДНИЙ выданный id, а не следующий:
    // выдача идёт как `next_id += 1; let id = next_id`. Инвариант написан по
    // поведению, а не по имени поля — иначе он падал бы на каждой первой сделке.
    // Само имя вводит в заблуждение и просится в переименование, но оно ездит
    // в хранилище (ADR-016), поэтому отдельной задачей.
    if let Some(max) = ids.last() {
        if *max > snap.next_id {
            v(
                &mut out,
                format!("выдан id {max}, больший счётчика next_id {}", snap.next_id),
            );
        }
    }

    out
}

// ==================== Тесты инвариантов (красная линия №5) ==================
//
// Проверки идут над слепком, поэтому и тесты — над слепком: тот же режим, в
// котором проверка работает в бою. Прошлая реализация жила в `PgStore`, а тесты
// шли на `NoopStore`, где она была пустой — и не выполнялась ни разу (TRM-06).
/// Пустой справочник цен для проверок ядра.
///
/// Пустой он намеренно: без цен нереализованный результат равен нулю (позиции
/// считаются по цене входа), то есть `equity == balance`, и свободная маржа
/// совпадает с прежней, балансовой. Значит существующие проверки продолжают
/// проверять **ровно то, что проверяли**, а не новое поведение заодно. Там, где
/// проверяется само новое поведение, цены задаются явно.
#[cfg(test)]
fn no_marks() -> HashMap<u32, i64> {
    HashMap::new()
}

#[cfg(test)]
mod invariant_tests {
    use super::*;

    fn pos(id: u64, qty: i64, entry: i64, margin: Cents) -> Position {
        Position {
            id,
            instrument: 1,
            side: PosSide::Long,
            qty,
            entry,
            pd: 2,
            qd: 3,
            margin,
            sl: None,
            tp: None,
            leverage: 1,
            fees: Default::default(),
        }
    }

    /// Маржа в фикстуре — не произвольная: она обязана сходиться с формулой
    /// `value_cents(entry, qty, pd, qd) / плечо`, иначе слепок «здоровым» не
    /// является. Первая версия этой фикстуры брала маржу с потолка, и новый
    /// инвариант поймал её сразу же — что и требовалось.
    fn healthy() -> AccountSnapshot {
        AccountSnapshot {
            profile: Default::default(),
            balance: 1_000_000,
            next_id: 2,
            positions: vec![
                pos(1, 100, 5000, value_minor(5000, 100, 2, 3, 100)),
                pos(2, 200, 4000, value_minor(4000, 200, 2, 3, 100)),
            ],
            pendings: vec![],
            closed: vec![],
            model: PositionModel::Hedging,
            leverage: None,
            fees: Default::default(),
        }
    }

    #[test]
    fn healthy_snapshot_has_no_violations() {
        assert_eq!(check_snapshot(&healthy()), vec![]);
    }

    #[test]
    fn empty_account_is_valid() {
        assert_eq!(check_snapshot(&AccountSnapshot::default()), vec![]);
    }

    /// Отрицательный баланс НЕ нарушение: с плечом он законен. Прежняя проверка
    /// считала его фатальным и не дала бы сервису стартовать (TRM-06).
    #[test]
    fn negative_balance_is_not_a_violation() {
        let mut s = healthy();
        s.balance = -50_000;
        assert_eq!(
            check_snapshot(&s),
            vec![],
            "отрицательный баланс законен с плечом"
        );
    }

    #[test]
    fn detects_zero_and_negative_qty() {
        let mut s = healthy();
        s.positions[0].qty = 0;
        assert!(
            check_snapshot(&s).iter().any(|v| v.0.contains("объём")),
            "нулевой объём должен ловиться"
        );
        s.positions[0].qty = -5;
        assert!(check_snapshot(&s).iter().any(|v| v.0.contains("объём")));
    }

    #[test]
    fn detects_zero_entry_price() {
        let mut s = healthy();
        s.positions[1].entry = 0;
        assert!(check_snapshot(&s)
            .iter()
            .any(|v| v.0.contains("цена входа")));
    }

    /// Маржа, разошедшаяся с пересчитанной, обязана ловиться: это единственный
    /// способ поймать значение, приехавшее из хранилища кривым или посчитанное
    /// при другом плече.
    #[test]
    fn detects_margin_drift() {
        let mut s = healthy();
        s.positions[0].margin += 1;
        assert!(
            check_snapshot(&s).iter().any(|v| v.0.contains("маржа")),
            "дрейф маржи должен ловиться"
        );
    }

    /// Маржа, не сходящаяся с плечом САМОЙ позиции, — нарушение.
    ///
    /// Раньше этот тест сверял слепок с плечом площадки, и смена настройки
    /// красила его у всех счетов разом (TRM-53 шаг 2). Теперь он ловит то, что
    /// и должен был: запись говорит «плечо 10», а маржа под ней посчитана под
    /// плечо 1 — значит в хранилище рассогласование, а не смена настройки.
    #[test]
    fn detects_margin_inconsistent_with_position_leverage() {
        let mut s = healthy();
        s.positions[0].leverage = 10;
        assert!(
            !check_snapshot(&s).is_empty(),
            "маржа под плечо 1 не годится для позиции, записанной с плечом 10"
        );
    }

    /// Смена плеча площадки НЕ двигает маржу уже открытой позиции (TRM-53).
    ///
    /// Это и есть денежный инвариант задачи. До шага 2 новая настройка
    /// пересчитывала маржу у всех открытых позиций разом: человек не трогал
    /// позицию, а занятое под ней менялось — в обе стороны.
    #[test]
    fn platform_leverage_change_leaves_open_margin_alone() {
        let mut b = Broker::new(1_000_000, 1);
        let u = UserId(1);
        b.open_account(u);
        let m = HashMap::from([(1u32, 5000i64)]);
        let id = b
            .open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
            .expect("позиция открыта");
        let before = b.used_margin(u);
        assert!(before > 0, "предпосылка: маржа занята");

        assert!(b.set_leverage(100), "плечо площадки сменилось");

        assert_eq!(
            b.used_margin(u),
            before,
            "смена настройки площадки не двигает занятое под открытой позицией"
        );
        assert_eq!(
            b.snapshot(u)
                .positions
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .leverage,
            1,
            "позиция помнит плечо, под которым открыта"
        );
        assert_eq!(
            check_snapshot(&b.snapshot(u)),
            vec![],
            "слепок сходится сам с собой и после смены настройки"
        );
    }

    /// Доливка после смены плеча идёт по плечу ПОЗИЦИИ, а не площадки.
    ///
    /// Дыра нашлась мутацией: подмена `cur.leverage` на сегодняшнее плечо не
    /// красила ни одного теста из 134 — ни один не доливал позицию после
    /// смены настройки. А именно это и случится у первого же человека,
    /// державшего позицию в день правки: под мутацией его маржа упала бы с
    /// 10 000 до 100, то есть позиция оказалась бы обеспечена в сто раз хуже,
    /// чем он соглашался, и молча.
    #[test]
    fn adding_to_a_position_after_a_leverage_change_keeps_its_leverage() {
        let mut b = Broker::new(1_000_000, 1);
        let u = UserId(1);
        b.open_account(u);
        // Доливка бывает только при неттинге: в хеджинге второе открытие — это
        // ВТОРАЯ позиция, и она законно берёт сегодняшнее плечо.
        assert!(
            b.adopt_model(u, PositionModel::Netting),
            "предпосылка: счёт неттинговый"
        );
        let m = HashMap::from([(1u32, 5000i64)]);
        b.open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
            .expect("позиция открыта");
        let one = b.used_margin(u);

        assert!(b.set_leverage(100), "плечо площадки сменилось");
        b.open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
            .expect("доливка прошла");

        assert_eq!(
            b.snapshot(u).positions.len(),
            1,
            "предпосылка: это доливка одной позиции, а не вторая позиция"
        );
        assert_eq!(
            b.used_margin(u),
            one * 2,
            "долитая половина обеспечена тем же плечом, что и первая"
        );
        assert_eq!(
            check_snapshot(&b.snapshot(u)),
            vec![],
            "слепок сходится: маржа отвечает плечу, записанному в позиции"
        );
    }

    /// Позиции по разные стороны правки плеча несут РАЗНЫЕ плечи — и обе верны.
    ///
    /// Это не край, а обычный день при хеджинге: правку делают, когда у людей
    /// открыты позиции. Ровно поэтому плечо обязано лежать в позиции: одного
    /// числа на счёт не хватает, чтобы описать такой счёт честно.
    #[test]
    fn positions_opened_around_a_leverage_change_keep_their_own() {
        let mut b = Broker::new(1_000_000, 1);
        let u = UserId(1);
        b.open_account(u);
        let m = HashMap::from([(1u32, 5000i64)]);
        b.open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
            .expect("первая позиция");
        assert!(b.set_leverage(100));
        b.open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
            .expect("вторая позиция");

        let snap = b.snapshot(u);
        // Сортируем: порядок позиций в слепке не обещан, и утверждать его
        // здесь значило бы прибивать к тесту постороннее свойство.
        let mut levs: Vec<i128> = snap.positions.iter().map(|p| p.leverage).collect();
        levs.sort_unstable();
        assert_eq!(levs, vec![1, 100], "каждая позиция помнит своё плечо");
        assert_eq!(
            b.used_margin(u),
            5050,
            "занято 5000 под первой (плечо 1) и 50 под второй (плечо 100)"
        );
        assert_eq!(
            check_snapshot(&snap),
            vec![],
            "счёт с двумя плечами — здоровый счёт, а не карантин"
        );
    }

    /// Плечо площадки не становится нулевым или отрицательным.
    ///
    /// Ноль уронил бы расчёт делением, отрицательное дало бы отрицательную
    /// маржу — то есть позицию, которая освобождает деньги вместо того чтобы
    /// их занимать. Отказ виден вызывающему: `false`, и значение прежнее.
    #[test]
    fn set_leverage_refuses_nonpositive() {
        let mut b = Broker::new(1_000_000, 7);
        for bad in [0i128, -5] {
            assert!(!b.set_leverage(bad), "плечо {bad} не принимается");
            assert_eq!(b.leverage(), 7, "после отказа плечо прежнее");
        }
        assert!(b.set_leverage(50), "нормальное значение принимается");
        assert_eq!(b.leverage(), 50);
    }

    /// Порог принудительного закрытия — отношение, а не сумма: 50 % остаются
    /// 50 % и при плече 1, и при плече 100 (формулировка штаба, TRM-53).
    #[test]
    fn margin_level_half_is_half_at_any_leverage() {
        // Занятое под одной и той же позицией при двух плечах — числами, а не
        // через тот же расчёт: 239883-подобной арифметики здесь нет, нотионал
        // 5000 × 1000 × 100 / 10^5 = 5000 наименьших единиц.
        for (lev, want_used) in [(1i128, 5000i128), (100, 50)] {
            let mut b = Broker::new(1_000_000, lev);
            let u = UserId(1);
            b.open_account(u);
            let m = HashMap::from([(1u32, 5000i64)]);
            b.open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
                .expect("позиция открыта");
            let used = b.used_margin(u);
            assert_eq!(
                used, want_used,
                "плечо {lev}: занято должно быть {want_used}"
            );
            // Уводим equity ровно в половину занятого: баланс + открытый P&L.
            let want_equity = used / 2;
            let bal = b.balance(u).expect("счёт заведён");
            let need_pnl = want_equity - bal;
            // P&L длинной = (mark - entry) * объём в минорных единицах счёта.
            let unit = b.open_pnl(u, &HashMap::from([(1u32, 5001i64)]));
            assert!(unit != 0, "предпосылка: тик цены двигает P&L");
            let mark = 5000 + (need_pnl / unit) as i64;
            let marks = HashMap::from([(1u32, mark)]);
            let pct = b.margin_level_pct(u, &marks).expect("уровень маржи есть");
            assert!(
                (pct - 50).abs() <= 1,
                "плечо {lev}: уровень маржи {pct}%, ожидали ~50%"
            );
        }
    }

    #[test]
    fn detects_negative_margin() {
        let mut s = healthy();
        s.positions[0].margin = -1;
        assert!(check_snapshot(&s)
            .iter()
            .any(|v| v.0.contains("отрицательная маржа")));
    }

    #[test]
    fn detects_duplicate_ids() {
        let mut s = healthy();
        s.positions[1].id = s.positions[0].id;
        assert!(check_snapshot(&s)
            .iter()
            .any(|v| v.0.contains("не уникальны")));
    }

    /// Счётчик id обязан покрывать выданные: иначе следующая позиция затрёт
    /// существующую, и одна из них исчезнет вместе со своей маржой.
    #[test]
    fn detects_id_beyond_counter() {
        let mut s = healthy();
        s.next_id = 1; // а позиция с id=2 уже есть
        assert!(check_snapshot(&s).iter().any(|v| v.0.contains("next_id")));
    }

    #[test]
    fn detects_bad_pending_order() {
        let mut s = healthy();
        s.next_id = 3;
        s.pendings.push(PendingOrder {
            id: 3,
            instrument: 1,
            side: PosSide::Long,
            qty: 0,
            price: 100,
            pd: 2,
            qd: 3,
            sl: None,
            tp: None,
        });
        assert!(check_snapshot(&s).iter().any(|v| v.0.contains("ордер")));
        s.pendings[0].qty = 10;
        s.pendings[0].price = 0;
        assert!(check_snapshot(&s).iter().any(|v| v.0.contains("цена")));
    }

    /// Инварианты обязаны считаться и без хранилища: они чистые функции.
    /// Этот тест — растяжка: если проверку снова унесут в БД, он покраснеет.
    #[test]
    fn invariants_do_not_need_a_store() {
        let b = Broker::new(1_000_000, 1);
        let snap = b.snapshot(UserId(42)); // счёта нет — слепок по умолчанию
        assert_eq!(check_snapshot(&snap), vec![]);
    }

    /// Настоящая торговая последовательность не нарушает инвариантов: открытие,
    /// отложенный ордер, закрытие. Проверяем на живом брокере, а не на руками
    /// собранном слепке — иначе проверка знала бы только о придуманных случаях.
    #[test]
    fn real_trading_sequence_keeps_invariants() {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        let id = b
            .open(
                u,
                &no_marks(),
                1,
                PosSide::Long,
                1000,
                50_000,
                2,
                3,
                None,
                None,
            )
            .expect("открытие");
        assert_eq!(check_snapshot(&b.snapshot(u)), vec![], "после открытия");
        b.place_pending(u, 2, PosSide::Short, 500, 40_000, 2, 3, None, None)
            .expect("ордер поставлен");
        assert_eq!(
            check_snapshot(&b.snapshot(u)),
            vec![],
            "после лимитного ордера"
        );
        b.close(u, id, 51_000).expect("закрытие");
        assert_eq!(check_snapshot(&b.snapshot(u)), vec![], "после закрытия");
    }

    /// Занятая маржа обязана быть суммой маржи позиций — самый опасный дрейф:
    /// он даёт торговать на деньги, которых нет, и молчит об этом.
    #[test]
    fn used_margin_matches_sum_of_positions() {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            1000,
            50_000,
            2,
            3,
            None,
            None,
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            2,
            PosSide::Short,
            300,
            20_000,
            2,
            3,
            None,
            None,
        )
        .unwrap();
        let snap = b.snapshot(u);
        let sum: Cents = snap.positions.iter().map(|p| p.margin).sum();
        assert_eq!(
            b.used_margin(u),
            sum,
            "used_margin разошёлся с суммой маржи позиций"
        );
        assert_eq!(check_snapshot(&snap), vec![]);
    }
}

// ========================= Тесты неттинга (ADR-025) =========================
//
// Эталоны посчитаны вручную по формуле `value_cents = price·qty·100 / 10^(pd+qd)`,
// а не сняты с реализации. Отдельное внимание — ЧАСТИЧНОМУ сворачиванию: ветвление
// «свернуть или открыть» скучно, а расчёт маржи и реализованного P&L на неполном
// объёме — то место, где ошибка стоит денег и не падает.
#[cfg(test)]
mod netting_tests {
    use super::*;

    const PD: u8 = 2;
    const QD: u8 = 3;

    fn netting_broker() -> (Broker, UserId) {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        assert!(
            b.adopt_model(u, PositionModel::Netting),
            "пустой счёт обязан принять модель"
        );
        (b, u)
    }

    // ---- Денежная математика отдельно от ветвления --------------------------

    #[test]
    fn merged_entry_is_volume_weighted() {
        // (5000·100 + 5400·300) / 400 = 2 120 000 / 400 = 5300
        assert_eq!(merged_entry(100, 5000, 300, 5400), 5300);
    }

    /// Округление к ближайшему, а не отбрасыванием: отбрасывание смещает вход в одну
    /// сторону систематически, то есть на длинной дистанции дарит или отнимает деньги.
    #[test]
    fn merged_entry_rounds_to_nearest_not_down() {
        // (100·1 + 101·1) / 2 = 100.5 → 101, а не 100.
        assert_eq!(merged_entry(1, 100, 1, 101), 101);
        // (100·1 + 102·1) / 2 = 101 ровно.
        assert_eq!(merged_entry(1, 100, 1, 102), 101);
    }

    #[test]
    fn merged_entry_of_single_lot_is_that_price() {
        assert_eq!(merged_entry(0, 0, 50, 777), 777);
    }

    #[test]
    fn realized_pnl_uses_partial_qty_not_whole_position() {
        // Лонг, вход 5300, выход 5600, сворачиваем 150 из 400:
        // (5600−5300)·150·100 / 10^5 = 45 центов.
        assert_eq!(
            realized_on_close(PosSide::Long, 5300, 5600, 150, PD, QD, 100),
            45
        );
        // Шорт зеркально: прибыль при падении.
        assert_eq!(
            realized_on_close(PosSide::Short, 5600, 5300, 150, PD, QD, 100),
            45
        );
        assert_eq!(
            realized_on_close(PosSide::Short, 5300, 5600, 150, PD, QD, 100),
            -45
        );
    }

    // ---- Доливка -------------------------------------------------------------

    #[test]
    fn same_side_averages_entry_and_keeps_one_position() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            300,
            5400,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();

        let ps = b.positions(u);
        assert_eq!(
            ps.len(),
            1,
            "неттинг обязан держать одну позицию на инструмент"
        );
        assert_eq!(ps[0].qty, 400);
        assert_eq!(ps[0].entry, 5300, "вход обязан быть средневзвешенным");
        assert_eq!(
            ps[0].margin, 2120,
            "маржа обязана пересчитаться на весь объём"
        );
        assert_eq!(b.used_margin(u), 2120);
    }

    /// Доливка не должна молча стирать выставленную защиту.
    #[test]
    fn adding_to_position_keeps_existing_sl_tp_when_not_given() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            Some(4900),
            Some(5500),
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5200,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        let ps = b.positions(u);
        assert_eq!(ps[0].sl, Some(4900), "SL не должен теряться при доливке");
        assert_eq!(ps[0].tp, Some(5500), "TP не должен теряться при доливке");
    }

    // ---- Частичное сворачивание — самая опасная часть ------------------------

    #[test]
    fn opposite_smaller_reduces_position_and_realizes_partial_pnl() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            400,
            5300,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        let before = b.balance(u).expect("счёт заведён");

        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            150,
            5600,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();

        let ps = b.positions(u);
        assert_eq!(ps.len(), 1, "позиция обязана остаться, а не удвоиться");
        assert_eq!(
            ps[0].side,
            PosSide::Long,
            "направление не меняется при частичном сворачивании"
        );
        assert_eq!(ps[0].qty, 250, "объём обязан уменьшиться на свёрнутое");
        assert_eq!(
            ps[0].entry, 5300,
            "вход не пересчитывается: свёрнута часть, а не заключена новая сделка"
        );
        assert_eq!(
            ps[0].margin, 1325,
            "маржа обязана уменьшиться пропорционально остатку"
        );
        assert_eq!(
            b.balance(u).expect("счёт заведён") - before,
            45,
            "реализован P&L только свёрнутой части"
        );

        let closed = b.closed_deals(u);
        assert_eq!(
            closed.len(),
            1,
            "в историю попадает свёрнутая часть, одной записью"
        );
        assert_eq!(closed[0].qty, 150, "в истории — фактически свёрнутый объём");
        assert_eq!(closed[0].exit, 5600);
        assert_eq!(closed[0].pnl, 45);
    }

    #[test]
    fn opposite_equal_closes_position_completely() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            200,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            200,
            5100,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();

        assert!(
            b.positions(u).is_empty(),
            "встречный равный объём обязан закрыть позицию"
        );
        assert_eq!(b.used_margin(u), 0, "маржа обязана освободиться полностью");
        assert_eq!(b.closed_deals(u).len(), 1);
    }

    /// Переворот: встречный объём больше открытого. В историю идёт только свёрнутая
    /// часть, остаток открывает позицию в новую сторону (ADR-025).
    #[test]
    fn opposite_larger_flips_position_and_records_only_the_closed_part() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        let before = b.balance(u).expect("счёт заведён");

        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            250,
            5200,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();

        let ps = b.positions(u);
        assert_eq!(ps.len(), 1, "после переворота остаётся одна позиция");
        assert_eq!(ps[0].side, PosSide::Short, "направление обязано смениться");
        assert_eq!(ps[0].qty, 150, "остаток = встречный объём минус свёрнутый");
        assert_eq!(ps[0].entry, 5200, "остаток открыт по цене встречной сделки");
        assert_eq!(ps[0].margin, 780);
        assert_eq!(
            b.balance(u).expect("счёт заведён") - before,
            20,
            "реализован P&L только закрытой части"
        );

        let closed = b.closed_deals(u);
        assert_eq!(
            closed.len(),
            1,
            "остаток ещё не закрыт — в историю он не попадает"
        );
        assert_eq!(
            closed[0].qty, 100,
            "в истории — только свёрнутый объём, не весь ордер"
        );
    }

    // ---- Инварианты и модель --------------------------------------------------

    #[test]
    fn netting_keeps_invariants_through_a_sequence() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            400,
            5300,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert_eq!(check_snapshot(&b.snapshot(u)), vec![], "после открытия");
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            150,
            5600,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            check_snapshot(&b.snapshot(u)),
            vec![],
            "после частичного сворачивания"
        );
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            500,
            5100,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert_eq!(check_snapshot(&b.snapshot(u)), vec![], "после переворота");
    }

    /// Разные инструменты не сворачиваются друг о друга.
    #[test]
    fn netting_is_per_instrument() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            2,
            PosSide::Short,
            100,
            3000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            b.positions(u).len(),
            2,
            "позиции по разным инструментам независимы"
        );
    }

    /// Хеджирование не изменилось: встречная сделка по-прежнему заводит вторую позицию.
    /// Растяжка — если неттинг «протечёт» в модель по умолчанию, тест покраснеет.
    #[test]
    fn hedging_still_opens_a_second_position() {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            100,
            5200,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            b.positions(u).len(),
            2,
            "хеджирование обязано остаться прежним"
        );
        assert!(
            b.closed_deals(u).is_empty(),
            "в хеджировании встречная сделка ничего не закрывает"
        );
    }

    #[test]
    fn default_model_is_hedging() {
        let b = Broker::new(10_000_000, 1);
        assert_eq!(
            b.model(UserId(1)),
            PositionModel::Hedging,
            "счёт без записи — хеджирующий"
        );
    }

    /// Вариант A: модель меняется только на «плоском» счёте.
    #[test]
    fn model_changes_only_when_flat() {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert!(
            !b.adopt_model(u, PositionModel::Netting),
            "с открытой позицией модель менять нельзя"
        );
        assert_eq!(
            b.model(u),
            PositionModel::Hedging,
            "счёт обязан продолжить работать по прежней"
        );

        let id = b.positions(u)[0].id;
        b.close(u, id, 5100).unwrap();
        assert!(
            b.adopt_model(u, PositionModel::Netting),
            "на плоском счёте модель принимается"
        );
        assert_eq!(b.model(u), PositionModel::Netting);
    }

    /// Отложенные ордера не мешают смене модели: они ещё не позиции, риска не несут.
    #[test]
    fn pending_orders_do_not_block_model_change() {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        b.place_pending(u, 1, PosSide::Long, 100, 4000, PD, QD, None, None)
            .expect("ордер поставлен");
        assert!(
            b.adopt_model(u, PositionModel::Netting),
            "ордер — не позиция"
        );
    }

    /// Модель переживает сохранение и восстановление: иначе после рестарта счёт
    /// молча сменил бы правила торговли.
    #[test]
    fn model_survives_snapshot_roundtrip() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        let snap = b.snapshot(u);
        assert_eq!(snap.model, PositionModel::Netting);

        let mut restored = Broker::new(10_000_000, 1);
        restored.restore(u, snap);
        assert_eq!(
            restored.model(u),
            PositionModel::Netting,
            "модель обязана пережить рестарт"
        );
        // И поведение восстановленного счёта — неттинговое, а не только поле.
        restored
            .open(
                u,
                &no_marks(),
                1,
                PosSide::Short,
                100,
                5100,
                PD,
                QD,
                None,
                None,
            )
            .unwrap();
        assert!(
            restored.positions(u).is_empty(),
            "восстановленный счёт обязан неттить"
        );
    }
}

// ============== Плечо счёта и заморозка на исполнении (Р-446) ===============
#[cfg(test)]
mod account_leverage_tests {
    use super::journal::{self, Journal, Origin};
    use super::*;

    const U: UserId = UserId(7);

    /// Брокер с открытым счётом и журналом, в который принята запись о заведении.
    fn setup(platform_lev: i128) -> (Broker, Journal) {
        let mut b = Broker::new(1_000_000_000, platform_lev);
        assert!(b.open_account(U));
        let mut j = Journal::new(b.start_balance());
        commit(&mut b, &mut j);
        (b, j)
    }

    /// Забрать события операции, свериться со слепком и принять их в ленту —
    /// тот же порядок, что у шлюза.
    fn commit(b: &mut Broker, j: &mut Journal) {
        let kinds = b.take_events(U);
        let sealed = j.seal(1, &Origin::System, &kinds);
        let snap = b.snapshot(U);
        assert_eq!(
            journal::reconcile(&j.preview(&sealed), &snap),
            vec![],
            "лента сходится со слепком"
        );
        for e in sealed {
            j.commit(e).expect("запись продолжает ленту");
        }
    }

    fn marks(px: i64) -> HashMap<u32, i64> {
        HashMap::from([(1u32, px)])
    }

    fn lev_of(b: &Broker, id: u64) -> i128 {
        b.positions(U)
            .into_iter()
            .find(|p| p.id == id)
            .expect("позиция есть")
            .leverage
    }

    #[test]
    fn unassigned_account_trades_at_platform_leverage() {
        let (mut b, mut j) = setup(20);
        assert_eq!(b.assigned_leverage(U), None);
        assert_eq!(b.account_leverage(U), 20, "умолчание — плечо площадки");
        let id = b
            .open(
                U,
                &marks(5000),
                1,
                PosSide::Long,
                1000,
                5000,
                2,
                3,
                None,
                None,
            )
            .unwrap();
        commit(&mut b, &mut j);
        assert_eq!(lev_of(&b, id), 20);
    }

    /// Новое плечо — новым позициям; старая держит снимок. Смена журналируется
    /// и переживает рестарт: свёртка ленты даёт то же плечо, что слепок.
    #[test]
    fn new_position_takes_new_leverage_old_keeps_its_own_and_survives_restart() {
        let (mut b, mut j) = setup(20);
        let old = b
            .open(
                U,
                &marks(5000),
                1,
                PosSide::Long,
                1000,
                5000,
                2,
                3,
                None,
                None,
            )
            .unwrap();
        commit(&mut b, &mut j);
        let old_margin = b.used_margin(U);

        assert_eq!(b.set_account_leverage(U, 50), Ok(true));
        assert_eq!(
            b.take_events_peek(),
            vec![EventKind::LeverageSet { lev: 50 }],
            "смена плеча — своя запись"
        );
        commit(&mut b, &mut j);
        assert_eq!(
            b.used_margin(U),
            old_margin,
            "маржа открытой позиции не двинулась"
        );

        let new = b
            .open(
                U,
                &marks(5000),
                1,
                PosSide::Long,
                1000,
                5000,
                2,
                3,
                None,
                None,
            )
            .unwrap();
        commit(&mut b, &mut j);
        assert_eq!(lev_of(&b, old), 20, "старая позиция — со старым плечом");
        assert_eq!(lev_of(&b, new), 50, "новая — с плечом счёта");

        // «Рестарт»: лента поднимается из записей, слепок — из хранилища.
        let events = j.events().to_vec();
        let reloaded = Journal::load(b.start_balance(), events).expect("лента цела");
        assert_eq!(reloaded.state().leverage, Some(50), "плечо в свёртке");
        let snap = b.snapshot(U);
        assert_eq!(snap.leverage, Some(50), "плечо в слепке");
        assert!(
            journal::audit(&reloaded, &snap).ok(),
            "сверка после рестарта"
        );
        let mut b2 = Broker::new(b.start_balance(), 20);
        b2.restore(U, snap);
        assert_eq!(
            b2.account_leverage(U),
            50,
            "после подъёма плечо счёта прежнее"
        );
    }

    /// Плечо в слепке без записи в ленте — расхождение, а не тихая правка.
    #[test]
    fn leverage_without_a_journal_entry_is_a_divergence() {
        let (mut b, j) = setup(20);
        b.set_account_leverage(U, 50).unwrap();
        let _ = b.take_events(U); // запись «потерялась»
        let ds = journal::reconcile(j.state(), &b.snapshot(U));
        assert!(
            ds.iter().any(|d| d.0.contains("плечо счёта")),
            "расхождение названо: {ds:?}"
        );
    }

    #[test]
    fn leverage_is_validated_and_a_repeat_writes_nothing() {
        let (mut b, _j) = setup(20);
        for bad in [0, -1, MAX_LEVERAGE + 1] {
            assert_eq!(
                b.set_account_leverage(U, bad),
                Err(BrokerError::InvalidLeverage),
                "плечо {bad} не принимается"
            );
        }
        assert_eq!(b.set_account_leverage(U, MAX_LEVERAGE), Ok(true));
        let _ = b.take_events(U);
        assert_eq!(b.set_account_leverage(U, MAX_LEVERAGE), Ok(false));
        assert_eq!(b.pending_events(U), 0, "повтор не пишет запись");
        assert_eq!(
            b.set_account_leverage(UserId(999), 10),
            Err(BrokerError::UnknownAccount),
            "счёт не заводится ради плеча"
        );
    }

    /// Отложенный ордер исполняется по плечу счёта НА МОМЕНТ ИСПОЛНЕНИЯ:
    /// маржу он не резервирует, допуск идёт при исполнении.
    #[test]
    fn pending_order_executes_at_the_leverage_in_force_when_it_fires() {
        let (mut b, mut j) = setup(20);
        b.place_pending(U, 1, PosSide::Long, 1000, 4900, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        b.set_account_leverage(U, 5).unwrap();
        commit(&mut b, &mut j);

        assert_eq!(b.check(&marks(4890), false), vec![U]);
        commit(&mut b, &mut j);
        let p = b.positions(U);
        assert_eq!(p.len(), 1, "ордер исполнен");
        assert_eq!(p[0].leverage, 5, "плечо — действующее при исполнении");
    }

    /// Дыра заморозки: ордер, поставленный до заморозки, при срабатывании
    /// позицию НЕ открывает — снимается записью с причиной.
    #[test]
    fn frozen_account_pending_is_rejected_on_fire_not_executed() {
        let (mut b, mut j) = setup(20);
        let id = b
            .place_pending(U, 1, PosSide::Long, 1000, 4900, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);

        let frozen = HashSet::from([U]);
        assert_eq!(b.check_with_frozen(&marks(4890), false, &frozen), vec![U]);
        assert_eq!(
            b.take_events_peek(),
            vec![EventKind::PendingRejected {
                id,
                exec: Some(4890),
                reason: RejectReason::AccountFrozen,
            }]
        );
        commit(&mut b, &mut j);
        assert!(b.positions(U).is_empty(), "позиция не открыта");
        assert!(b.pendings(U).is_empty(), "ордер снят");
        assert!(j.state().pendings.is_empty(), "и в свёртке тоже");
    }

    /// Не сработавший ордер замороженного счёта остаётся стоять: снимается он
    /// только при срабатывании, разморозка до него — и он исполнится.
    #[test]
    fn frozen_account_pending_that_does_not_fire_stays() {
        let (mut b, mut j) = setup(20);
        b.place_pending(U, 1, PosSide::Long, 1000, 4900, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        let frozen = HashSet::from([U]);
        assert!(b.check_with_frozen(&marks(5000), false, &frozen).is_empty());
        assert_eq!(b.pendings(U).len(), 1);
    }

    impl Broker {
        /// Посмотреть события очереди, не забирая их.
        fn take_events_peek(&self) -> Vec<EventKind> {
            self.accounts
                .get(&U)
                .map(|a| a.outbox.clone())
                .unwrap_or_default()
        }
    }
}

// ============== Тариф счёта: наценка и комиссия (решение 10.11) ==============
#[cfg(test)]
mod fees_tests {
    use super::journal::{self, Journal, Origin};
    use super::*;

    const U: UserId = UserId(9);
    /// 50 000.00 при pd=2.
    const PX: i64 = 5_000_000;
    /// 1.000 при qd=3.
    const Q: i64 = 1000;

    fn setup() -> (Broker, Journal) {
        setup_with(Broker::new(1_000_000_000, 20))
    }

    fn setup_with(mut b: Broker) -> (Broker, Journal) {
        assert!(b.open_account(U));
        let mut j = Journal::new(b.start_balance());
        commit(&mut b, &mut j);
        (b, j)
    }

    /// Тот же порядок, что у шлюза: забрать события, свериться, принять.
    fn commit(b: &mut Broker, j: &mut Journal) -> Vec<EventKind> {
        let kinds = b.take_events(U);
        let sealed = j.seal(1, &Origin::System, &kinds);
        let snap = b.snapshot(U);
        assert_eq!(
            journal::reconcile(&j.preview(&sealed), &snap),
            vec![],
            "лента сходится со слепком"
        );
        for e in sealed {
            j.commit(e).expect("запись продолжает ленту");
        }
        kinds
    }

    fn marks(px: i64) -> HashMap<u32, i64> {
        HashMap::from([(1u32, px)])
    }

    fn fees(markup_bps: i64, commission_bps: i64) -> Fees {
        Fees {
            markup_bps,
            commission_bps,
        }
    }

    fn open(b: &mut Broker, side: PosSide, market: i64) -> Result<u64, BrokerError> {
        b.open(U, &marks(market), 1, side, Q, market, 2, 3, None, None)
    }

    fn pos(b: &Broker, id: u64) -> Position {
        b.positions(U).into_iter().find(|p| p.id == id).unwrap()
    }

    #[test]
    fn helpers_round_to_nearest() {
        // 10 bps от 50 000.00 = 50.00 → 5000 единиц цены.
        assert_eq!(apply_markup(true, PX, 10), PX + 5000);
        assert_eq!(apply_markup(false, PX, 10), PX - 5000);
        assert_eq!(apply_markup(true, PX, 0), PX, "ноль — рынок");
        // 1 bps от 15 = 0.0015 → 0; от 5000 = 0.5 → половина от нуля = 1.
        assert_eq!(apply_markup(true, 15, 1), 15);
        assert_eq!(apply_markup(true, 5000, 1), 5001);
        // Нотионал 50 050.00 USD = 5 005 000 центов; 7 bps = 3503.5 → 3504.
        assert_eq!(commission_for(5_005_000, Q, 2, 3, 7, 100), 3504);
        assert_eq!(commission_for(PX, Q, 2, 3, 0, 100), 0);
        assert_eq!(markup_amount(PX, PX + 5000, Q, 2, 3, 100), 5000);
        assert_eq!(markup_amount(PX, PX - 5000, Q, 2, 3, 100), 5000);
    }

    /// Покупка — дороже рынка, продажа — дешевле; на открытии и на закрытии,
    /// для обеих сторон.
    #[test]
    fn markup_moves_the_price_against_the_client_on_both_sides() {
        let (mut b, mut j) = setup();
        b.set_account_fees(U, fees(10, 0)).unwrap();
        commit(&mut b, &mut j);

        let long = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        assert_eq!(pos(&b, long).entry, PX + 5000, "покупка дороже ask");
        let short = open(&mut b, PosSide::Short, PX).unwrap();
        commit(&mut b, &mut j);
        assert_eq!(pos(&b, short).entry, PX - 5000, "продажа дешевле bid");

        b.close(U, long, PX).unwrap();
        commit(&mut b, &mut j);
        b.close(U, short, PX).unwrap();
        let kinds = commit(&mut b, &mut j);
        let deals = b.closed_deals(U);
        assert_eq!(deals[0].exit, PX - 5000, "закрытие лонга — продажа дешевле");
        assert_eq!(deals[1].exit, PX + 5000, "закрытие шорта — покупка дороже");
        for d in &deals {
            assert_eq!(
                d.pnl, -10_000,
                "рынок не двигался — клиент теряет обе наценки"
            );
            assert_eq!(d.fees.markup, 10_000, "наценка входа и выхода в сделке");
            assert_eq!(d.fees.commission, 0);
            assert_eq!(d.fees.market_exit, Some(PX), "рыночная цена закрытия");
        }
        assert!(kinds.contains(&EventKind::MarkupApplied {
            trade: short,
            mkt: PX,
            exec: PX + 5000,
            amount: 5000
        }));
        assert_eq!(b.balance(U), Some(1_000_000_000 - 20_000));
    }

    /// Комиссия списывается на открытии и на закрытии, отдельными проводками,
    /// и вся отнесена к сделке.
    #[test]
    fn commission_is_charged_on_open_and_on_close() {
        let (mut b, mut j) = setup();
        b.set_account_fees(U, fees(0, 7)).unwrap();
        assert_eq!(
            commit(&mut b, &mut j),
            vec![EventKind::FeesSet {
                markup_bps: 0,
                commission_bps: 7
            }]
        );
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        let kinds = commit(&mut b, &mut j);
        // Нотионал 50 000.00 = 5 000 000 центов · 7 bps = 3500.
        assert_eq!(
            kinds.last(),
            Some(&EventKind::CommissionCharged {
                amount: 3500,
                trade: id,
                waived: 0
            })
        );
        assert_eq!(b.balance(U), Some(1_000_000_000 - 3500));
        assert_eq!(pos(&b, id).fees.commission, 3500);

        // Закрытие по 51 000.00: нотионал 5 100 000 · 7 bps = 3570.
        b.close(U, id, 5_100_000).unwrap();
        let kinds = commit(&mut b, &mut j);
        assert!(matches!(
            kinds.as_slice(),
            [
                EventKind::CommissionCharged {
                    amount: 3570,
                    waived: 0,
                    ..
                },
                EventKind::PositionClosed { .. }
            ]
        ));
        let d = &b.closed_deals(U)[0];
        assert_eq!(d.pnl, 100_000, "результат — по цене, без комиссии");
        assert_eq!(d.fees.commission, 3500 + 3570, "комиссия входа и выхода");
        assert_eq!(b.balance(U), Some(1_000_000_000 + 100_000 - 7070));
    }

    /// Допуск учитывает комиссию: на маржу хватает, на маржу и комиссию — нет.
    #[test]
    fn admission_counts_the_commission() {
        // Маржа 1:20 под 50 000.00 = 250 000; комиссия 5 bps = 2500.
        let (mut b, _j) = setup_with(Broker::new(251_000, 20));
        b.set_account_fees(U, fees(0, 5)).unwrap();
        assert_eq!(
            open(&mut b, PosSide::Long, PX),
            Err(BrokerError::InsufficientMargin)
        );
        b.set_account_fees(U, fees(0, 1)).unwrap(); // 500 — влезает
        assert!(open(&mut b, PosSide::Long, PX).is_ok());
    }

    /// Стоп-аут закрывает с комиссией.
    #[test]
    fn stop_out_pays_commission() {
        let (mut b, mut j) = setup_with(Broker::new(300_000, 20).with_stop_out_pct(Some(50)));
        b.set_account_fees(U, fees(0, 10)).unwrap();
        commit(&mut b, &mut j);
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        // Падение на 4 %: убыток 200 000, уровень маржи ниже 50 % — стоп-аут.
        let px = 4_800_000;
        assert_eq!(b.check(&marks(px), false), vec![U]);
        let kinds = commit(&mut b, &mut j);
        assert!(b.positions(U).is_empty(), "закрыта стоп-аутом");
        assert!(
            kinds.contains(&EventKind::CommissionCharged {
                amount: commission_for(px, Q, 2, 3, 10, 100),
                trade: id,
                waived: 0
            }),
            "{kinds:?}"
        );
        assert!(kinds.iter().any(|k| matches!(
            k,
            EventKind::PositionClosed {
                cause: CloseCause::StopOut,
                ..
            }
        )));
    }

    /// Стоп-лосс закрывает с комиссией.
    #[test]
    fn stop_loss_pays_commission() {
        let (mut b, mut j) = setup();
        b.set_account_fees(U, fees(0, 10)).unwrap();
        commit(&mut b, &mut j);
        let id = b
            .open(
                U,
                &marks(PX),
                1,
                PosSide::Long,
                Q,
                PX,
                2,
                3,
                Some(4_900_000),
                None,
            )
            .unwrap();
        commit(&mut b, &mut j);
        b.check(&marks(4_890_000), false);
        let kinds = commit(&mut b, &mut j);
        assert!(kinds.contains(&EventKind::CommissionCharged {
            amount: commission_for(4_890_000, Q, 2, 3, 10, 100),
            trade: id,
            waived: 0
        }));
        assert_eq!(b.closed_deals(U).len(), 1);
    }

    /// Отложенный ордер исполняется по тарифу, действующему при ИСПОЛНЕНИИ
    /// (как с плечом), и платит комиссию.
    #[test]
    fn pending_order_pays_at_the_tariff_in_force_when_it_fires() {
        let (mut b, mut j) = setup();
        b.place_pending(U, 1, PosSide::Long, Q, 4_900_000, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        b.set_account_fees(U, fees(10, 3)).unwrap();
        commit(&mut b, &mut j);
        b.check(&marks(4_890_000), false);
        let kinds = commit(&mut b, &mut j);
        let p = b.positions(U)[0].clone();
        assert_eq!(p.entry, 4_890_000 + 4890, "наценка на исполнении ордера");
        assert_eq!(p.fees.rates(), fees(10, 3));
        assert!(kinds
            .iter()
            .any(|k| matches!(k, EventKind::CommissionCharged { trade, .. } if *trade == p.id)));
    }

    /// Смена тарифа не трогает открытые позиции: закрытие — по снимку.
    /// Тариф переживает рестарт: свёртка ленты сходится со слепком.
    #[test]
    fn open_positions_keep_their_tariff_and_everything_survives_a_restart() {
        let (mut b, mut j) = setup();
        b.set_account_fees(U, fees(10, 5)).unwrap();
        commit(&mut b, &mut j);
        let old = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        b.set_account_fees(U, fees(0, 0)).unwrap();
        commit(&mut b, &mut j);
        let new = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        assert_eq!(
            pos(&b, old).fees.rates(),
            fees(10, 5),
            "старая — свой тариф"
        );
        assert_eq!(pos(&b, new).fees.rates(), fees(0, 0), "новая — тариф счёта");
        assert_eq!(pos(&b, new).entry, PX, "без наценки");

        // Рестарт посреди жизни позиций.
        let snap = b.snapshot(U);
        let reloaded = Journal::load(b.start_balance(), j.events().to_vec()).unwrap();
        assert!(
            journal::audit(&reloaded, &snap).ok(),
            "сверка после рестарта"
        );
        assert_eq!(reloaded.state().fees, fees(0, 0));
        let mut b2 = Broker::new(b.start_balance(), 20);
        b2.restore(U, snap);
        let mut j2 = reloaded;

        b2.close(U, old, PX).unwrap();
        commit(&mut b2, &mut j2);
        b2.close(U, new, PX).unwrap();
        commit(&mut b2, &mut j2);
        let d = b2.closed_deals(U);
        assert_eq!(
            d[0].exit,
            PX - 5000,
            "старая закрыта с наценкой своего тарифа"
        );
        assert!(d[0].fees.commission > 0);
        assert_eq!(d[1].exit, PX, "новая — без наценки");
        assert_eq!(
            d[1].fees,
            DealFees {
                commission: 0,
                markup: 0,
                market_exit: Some(PX)
            }
        );
        assert!(journal::audit(&j2, &b2.snapshot(U)).ok());
        let replayed = j2.replay();
        assert_eq!(replayed.closed[0].fees.market_exit, Some(PX));
        assert_eq!(replayed.closed[0].fees.markup, d[0].fees.markup);
    }

    /// Неттинг: доливка и сворачивание — по снимку позиции, остаток
    /// разворота — по тарифу счёта; свёртка сходится на каждом шаге.
    #[test]
    fn netting_reduce_increase_and_flip_reconcile() {
        let (mut b, mut j) = setup();
        assert!(b.adopt_model(U, PositionModel::Netting));
        b.set_account_fees(U, fees(10, 5)).unwrap();
        commit(&mut b, &mut j);
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        b.set_account_fees(U, fees(20, 1)).unwrap();
        commit(&mut b, &mut j);
        // Доливка — по снимку (10 bps).
        open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        assert_eq!(pos(&b, id).entry, PX + 5000);
        // Частичное сворачивание.
        b.open(U, &marks(PX), 1, PosSide::Short, 500, PX, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        assert_eq!(b.closed_deals(U)[0].exit, PX - 5000, "по снимку позиции");
        assert!(b.closed_deals(U)[0].fees.commission > 0);
        assert_eq!(pos(&b, id).fees.commission, 0, "накопленное ушло в сделку");
        // Разворот: 1.5 закрывает остаток, 0.5 открывает шорт по тарифу счёта.
        let new = b
            .open(U, &marks(PX), 1, PosSide::Short, 2000, PX, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        assert_ne!(new, id);
        assert_eq!(pos(&b, new).entry, PX - 10_000, "20 bps — тариф счёта");
        assert_eq!(pos(&b, new).fees.rates(), fees(20, 1));
        assert!(journal::audit(&j, &b.snapshot(U)).ok());
    }

    /// Защита от минуса: комиссия закрытия не уводит остаток ниже нуля —
    /// непогашенная часть названа (`waived`), убыток компенсируется как раньше.
    #[test]
    fn negative_balance_protection_caps_the_commission() {
        let (mut b, mut j) =
            setup_with(Broker::new(300_000, 20).with_negative_balance_protection(true));
        b.set_account_fees(U, fees(0, 10)).unwrap();
        commit(&mut b, &mut j);
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        let before = b.balance(U).unwrap();
        // Обвал на 10 %: убыток 500 000 больше остатка.
        b.close(U, id, 4_500_000).unwrap();
        b.check(&marks(4_500_000), false);
        let kinds = commit(&mut b, &mut j);
        let due = commission_for(4_500_000, Q, 2, 3, 10, 100);
        assert!(
            kinds.contains(&EventKind::CommissionCharged {
                amount: 0,
                trade: id,
                waived: due
            }),
            "{kinds:?}"
        );
        assert_eq!(b.balance(U), Some(0), "защита погасила минус");
        assert!(kinds.contains(&EventKind::NegativeBalanceCompensated {
            amount: 500_000 - before
        }));
    }

    /// Без защиты комиссия списывается целиком — минус виден проводкой.
    #[test]
    fn without_protection_the_commission_is_charged_in_full() {
        let (mut b, mut j) = setup_with(Broker::new(300_000, 20));
        b.set_account_fees(U, fees(0, 10)).unwrap();
        commit(&mut b, &mut j);
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        b.close(U, id, 4_500_000).unwrap();
        let kinds = commit(&mut b, &mut j);
        let due = commission_for(4_500_000, Q, 2, 3, 10, 100);
        assert!(kinds.contains(&EventKind::CommissionCharged {
            amount: due,
            trade: id,
            waived: 0
        }));
        assert!(b.balance(U).unwrap() < 0);
    }

    #[test]
    fn fees_are_validated_journaled_and_a_repeat_writes_nothing() {
        let (mut b, j) = setup();
        let mut j = j;
        for bad in [
            fees(-1, 0),
            fees(0, -1),
            fees(MAX_FEE_BPS + 1, 0),
            fees(0, 501),
        ] {
            assert_eq!(b.set_account_fees(U, bad), Err(BrokerError::InvalidFees));
        }
        assert_eq!(
            b.set_account_fees(U, fees(MAX_FEE_BPS, MAX_FEE_BPS)),
            Ok(true)
        );
        commit(&mut b, &mut j);
        assert_eq!(
            b.set_account_fees(U, fees(MAX_FEE_BPS, MAX_FEE_BPS)),
            Ok(false)
        );
        assert_eq!(b.pending_events(U), 0);
        assert_eq!(
            b.set_account_fees(UserId(999), fees(1, 1)),
            Err(BrokerError::UnknownAccount)
        );
        // Тариф в слепке без записи — расхождение.
        b.set_account_fees(U, fees(1, 1)).unwrap();
        let _ = b.take_events(U);
        let ds = journal::reconcile(j.state(), &b.snapshot(U));
        assert!(ds.iter().any(|d| d.0.contains("тариф счёта")), "{ds:?}");
    }

    /// Тариф вне пределов, приехавший из хранилища, — нарушение инварианта
    /// (счёт в карантин), а не ставка, по которой пойдут деньги.
    #[test]
    fn a_tariff_out_of_range_in_storage_is_a_violation() {
        let (mut b, _j) = setup();
        open(&mut b, PosSide::Long, PX).unwrap();
        assert_eq!(check_snapshot(&b.snapshot(U)), vec![]);
        let mut snap = b.snapshot(U);
        snap.fees = fees(MAX_FEE_BPS + 1, 0);
        snap.positions[0].fees.commission_bps = -1;
        snap.positions[0].fees.markup = -5;
        let v = check_snapshot(&snap);
        assert_eq!(v.len(), 3, "{v:?}");
    }

    /// Без тарифа — ровно прежнее: ни одной новой записи, цена — рынок.
    #[test]
    fn zero_tariff_is_the_old_behaviour() {
        let (mut b, mut j) = setup();
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        let kinds = commit(&mut b, &mut j);
        assert_eq!(kinds.len(), 1, "только PositionOpened: {kinds:?}");
        assert_eq!(pos(&b, id).entry, PX);
        b.close(U, id, PX).unwrap();
        let kinds = commit(&mut b, &mut j);
        assert_eq!(kinds.len(), 1, "только PositionClosed: {kinds:?}");
        assert_eq!(b.balance(U), Some(1_000_000_000));
    }

    /// Запись тарифа и проводок — канон и разбор сходятся; мусор — отказ.
    #[test]
    fn fee_records_round_trip_and_garbage_is_refused() {
        for k in [
            EventKind::FeesSet {
                markup_bps: 15,
                commission_bps: 0,
            },
            EventKind::MarkupApplied {
                trade: 3,
                mkt: 100,
                exec: 101,
                amount: 7,
            },
            EventKind::CommissionCharged {
                amount: 5,
                trade: 3,
                waived: 0,
            },
            EventKind::CommissionCharged {
                amount: 0,
                trade: 3,
                waived: 9,
            },
        ] {
            assert_eq!(
                EventKind::decode(&k.encode()),
                Ok(k.clone()),
                "{}",
                k.encode()
            );
        }
        assert_eq!(
            EventKind::CommissionCharged {
                amount: 5,
                trade: 3,
                waived: 0
            }
            .encode(),
            "commission_charged amount=5 trade=3"
        );
        for bad in [
            "fees_set markup_bps=501 commission_bps=0",
            "fees_set markup_bps=-1 commission_bps=0",
            "commission_charged amount=-1 trade=3",
            "markup_applied trade=1 mkt=1 exec=2 amount=-3",
        ] {
            assert!(EventKind::decode(bad).is_err(), "{bad}");
        }
    }
}
