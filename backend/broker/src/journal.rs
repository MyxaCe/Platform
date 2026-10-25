//! # Журнал событий счёта (TRM-13, Р-049)
//!
//! Неизменяемая лента того, **что произошло со счётом**: открытие и закрытие позиции,
//! доливка, частичное сворачивание, срабатывание стопа, пополнение и вывод. Лента
//! пополняется только добавлением, и по ней состояние счёта **восстанавливается**:
//! [`Journal::replay`] сворачивает записи в [`ReplayState`], а [`reconcile`] сравнивает
//! результат со слепком [`AccountSnapshot`].
//!
//! ## Зачем это раньше функции исправления
//!
//! Центр управления (Р-049) получит право начислить деньги вручную и поправить сделку.
//! Исправлять состояние, которое нечем переиграть, — значит делать необратимые правки
//! в системе без истории. Поэтому журнал первый, а исправление (ADM-03) — после.
//!
//! ## Форма записи заранее вмещает ручную проводку
//!
//! [`Origin`] отвечает на «кто и почему»: событие породил движок ([`Origin::System`]),
//! владелец счёта ([`Origin::Client`]) или оператор ([`Origin::Operator`] — кто именно,
//! основание, ссылка на инцидент). Поля обязательные **по типу**: ручную проводку без
//! основания и инцидента нельзя даже собрать. Добавлять их потом значило бы переписывать
//! журнал и все уже записанные отпечатки.
//!
//! ## Чем держится неизменяемость
//!
//! Поля [`AccountEvent`] приватны, конструктора «собери любую запись» нет: номер, время
//! и отпечаток проставляет [`Journal::seal`]. Каждая запись несёт отпечаток предыдущей,
//! то есть **цепочку**: правка, вставка или удаление записи ломает её и ловится
//! [`Journal::verify`] — в том числе правка прямо в базе, мимо кода.
//!
//! Отпечаток — 128-битный FNV-1a по каноническому тексту записи. Он **обнаруживает
//! правку, но не защищает от подделки**: ключа нет, и тот, кто может переписать всю
//! цепочку целиком, пересчитает и отпечатки. Подпись секретом — отдельное решение, и
//! принимать его надо вместе с моделью доступа центра управления, а не здесь.
//!
//! ## Детерминизм (красная линия №2)
//!
//! Модуль не знает ни времени, ни I/O: `at_ms` приходит снаружи, вместе с событием.
//! Деньги считаются теми же функциями, что и в торговом ядре (`merged_entry`,
//! `margin_for`, `realized_on_close`) — одна формула на одни деньги. Независимо здесь
//! пересчитывается **ветвление**: какая операция чем обернулась. Именно его и проверяет
//! сверка, и именно там ошибка не падает, а тихо стоит денег.

use std::fmt::Write as _;

use domain::account::{
    AccountKind, AccountOrigin, AccountProfile, CategoryAtOpen, ClientCategory, Currency, UserId,
};

use crate::{
    margin_for, merged_entry, realized_on_close, AccountSnapshot, Cents, ClosedDeal, PendingOrder,
    PosSide, Position, PositionModel,
};

// ============================== Кто и почему ================================

/// Ручная проводка: кто именно, на каком основании, по какому инциденту.
///
/// Поля приватны и проверяются при создании: запись «оператор поправил, причина не
/// указана» бессмысленна ровно тогда, когда её будут читать — при разборе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operator {
    who: String,
    reason: String,
    incident: String,
}

/// Почему ручную проводку не приняли.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadOperator(pub &'static str);

impl std::fmt::Display for BadOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl Operator {
    /// Собрать ручную проводку. Пустое поле — отказ, а не пустая строка в журнале.
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

/// Источник события.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Движок: сработал стоп, тейк или отложенный ордер. Человека в этот момент нет.
    System,
    /// Владелец счёта через терминал.
    Client,
    /// Оператор центра управления — ручная проводка (Р-049, ADM-03).
    Operator(Operator),
}

impl Origin {
    /// Короткое имя источника для хранилища и выдачи.
    pub fn tag(&self) -> &'static str {
        match self {
            Origin::System => "system",
            Origin::Client => "client",
            Origin::Operator(_) => "operator",
        }
    }

    /// Оператор, если событие ручное.
    pub fn operator(&self) -> Option<&Operator> {
        match self {
            Origin::Operator(o) => Some(o),
            _ => None,
        }
    }
}

// ============================== Что произошло ===============================

/// Почему позиция закрыта целиком.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseCause {
    /// Закрыта по команде (владельцем счёта или оператором — это видно по [`Origin`]).
    Command,
    /// Сработал стоп-лосс.
    StopLoss,
    /// Сработал тейк-профит.
    TakeProfit,
    /// Свёрнута встречной сделкой целиком (неттинг, ADR-025).
    Netting,
    // ЗАПИСИ О МАРЖИН-КОЛЛЕ ЗДЕСЬ НЕТ, И ЭТО РЕШЕНИЕ, А НЕ ЗАБЫВЧИВОСТЬ
    // (TRM-50, подтверждено штабом 24.10).
    //
    // Журнал — лента, из которой выводится состояние; предупреждение состояния
    // не меняет, и запись о нём пришлось бы пропускать при свёртке. Такой вид
    // уже есть ровно один (`SandboxDeclared`), и условие штаба — второй это
    // новое решение, а не добавление строки.
    //
    // След у предупреждения есть, и он БОГАЧЕ журнального: строка `bus_outbox`
    // хранит и момент, и отметку отправки, то есть отвечает ещё и на «дошло
    // ли» — чего запись в журнале не умеет. Искать предупреждения надо там.
    /// **Закрыта принудительно: уровень маржи упал ниже порога** (TRM-49).
    ///
    /// Своя причина, а не `Command`: закрыл не человек и не его распоряжение, а
    /// риск-контроль. На претензию «почему мою позицию закрыли» это разные
    /// ответы, и склеенные они отправляют разбираться не туда.
    StopOut,
}

/// Изменение состояния счёта.
///
/// События несут **входные данные операции**, а не её результат: цена входа, маржа и
/// реализованный P&L пересчитываются при свёртке. Иначе сверка сравнивала бы значение
/// с самим собой — тавтология, которая уже один раз прошла у нас за проверку маржи.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// Счёт заведён: стартовый баланс и **неизменная часть профиля** (TRM-44).
    ///
    /// Профиль едет здесь, потому что заведение — единственный момент, когда он
    /// устанавливается, и журнал обязан объяснять не только «сколько денег
    /// появилось», но и **на каком счёте**.
    ///
    /// # Старые записи
    ///
    /// Поля добавлены 24.10, и записи, написанные раньше, их не несут. Они
    /// разбираются с [`AccountProfile::legacy_demo`] — демо, розничный, доллар,
    /// происхождение «регистрация». Это **не умолчание на всякий случай, а факт
    /// их истории**: другого в терминале не существовало.
    ///
    /// И главное: **их канон не меняется**. Запись подписана своим текстом и
    /// хранит его (см. `AccountEvent::canon`), поэтому расширение вида не
    /// трогает отпечатки прежних записей. Без этого добавление полей сюда
    /// отправило бы в карантин все существующие счета.
    AccountOpened {
        balance: Cents,
        profile: AccountProfile,
    },
    /// Счёт перешёл на модель ведения позиций (ADR-025).
    ModelAdopted { model: PositionModel },
    /// Пополнение. Ручное начисление центра управления — это оно же с [`Origin::Operator`].
    Deposited { amount: Cents },
    /// Вывод.
    Withdrawn { amount: Cents },
    /// Открыта новая позиция.
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
        /// Плечо, под которым позиция открыта (TRM-53).
        ///
        /// **Едет в записи, потому что маржа пересчитывается свёрткой.** Записи
        /// несут входные данные, а не результат; маржа считается как
        /// `нотионал / плечо`, значит плечо — такой же вход, как цена и объём.
        /// Без него свёртка считала бы старые позиции по сегодняшней настройке
        /// площадки, и смена плеча переписывала бы маржу задним числом у всех
        /// открытых позиций разом.
        ///
        /// У записей, написанных до появления поля, плечо равно **1**: другого
        /// в терминале не было. Их канон при этом не меняется — запись
        /// подписана своим текстом (`AccountEvent::canon`).
        lev: i128,
    },
    /// Доливка: к позиции добавлен объём по своей цене (неттинг).
    PositionIncreased {
        id: u64,
        add_qty: i64,
        add_entry: i64,
        sl: Option<i64>,
        tp: Option<i64>,
    },
    /// Частичное сворачивание: закрыта часть объёма по цене `exit`.
    PositionReduced { id: u64, qty: i64, exit: i64 },
    /// Позиция закрыта целиком.
    ///
    /// `exit` — цена, по которой **исполнено**: по ней считаются деньги. У
    /// стопа и тейка это **наблюдённая рыночная** цена (Р-178,
    /// проскальзывание). `level` — уровень, который ставил человек; у закрытия
    /// по команде уровня нет вовсе, там `None`.
    ///
    /// На спокойном рынке они совпадают, на разрыве расходятся — и эта разница
    /// и есть проскальзывание. Нужны обе: по одной `exit` не отличить «рынок
    /// дошёл ровно до стопа» от «рынок перепрыгнул стоп», а это разные
    /// разговоры с клиентом (TRM-31).
    ///
    /// Почему не гарантируем уровень: разницу тогда платит брокер — на каждом
    /// скачке и без предела. Гарантированный стоп это отдельный продукт,
    /// который продают за деньги, а не умолчание, о котором никто не знает.
    ///
    /// `across_gap` — закрытие случилось на первом тике после молчания фида,
    /// то есть ряд цены был разорван (Р-073).
    PositionClosed {
        id: u64,
        exit: i64,
        cause: CloseCause,
        level: Option<i64>,
        across_gap: bool,
    },
    /// Размещён отложенный ордер.
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
    /// Отложенный ордер снят.
    PendingCancelled { id: u64 },
    /// Отложенный ордер сработал: сам ордер исчез, а открытие позиции — отдельной записью.
    ///
    /// `price` — уровень самого ордера, `exec` — цена, по которой он
    /// действительно исполнен (наблюдённая рыночная, Р-178).
    ///
    /// Для лимитного ордера исполнение по рынку всегда **не хуже** его уровня:
    /// покупка срабатывает, когда рынок опустился до уровня или ниже, продажа
    /// — когда поднялся до уровня или выше. Разрыв здесь играет за клиента, и
    /// это то же правило, а не исключение из него.
    PendingTriggered {
        id: u64,
        price: i64,
        exec: Option<i64>,
        across_gap: bool,
    },
    /// **Демо-счёт сброшен к стартовому остатку** (TRM-54).
    ///
    /// `adjustment` — на сколько изменился остаток: со знаком, потому что
    /// сбросить можно и вниз. Отдельной статьёй, а не записью стартовой суммы:
    /// «стало 10 000» не отвечает, **сколько списали или добавили**, а по
    /// журналу обязано восстанавливаться движение, а не только итог. Та же
    /// причина, по которой именована компенсация.
    ///
    /// Владелец денежного числа — терминал (решение владельца 24.10). Кабинет
    /// эту запись не делает и делать не может: он отражает, а не пишет деньги.
    DemoAccountReset { adjustment: Cents },
    /// **Отрицательный остаток погашен за счёт брокера** (TRM-42).
    ///
    /// `amount` — сумма компенсации, ровно столько, сколько не хватало до нуля.
    /// Отдельной статьёй, а не записью нуля в баланс: «стало ноль» не отвечает
    /// на вопрос, **сколько мы доплатили**, а это наши деньги и их надо уметь
    /// сложить за период. Обнуление без суммы уничтожает единственную цифру,
    /// ради которой защита вообще считается (решение владельца, ADM-07).
    ///
    /// Состояние она МЕНЯЕТ — это движение денег, и свёртка его применяет. С
    /// `SandboxDeclared` её роднит только то, что обе появились недавно; к
    /// правилу «один вид, не меняющий состояние» эта запись отношения не имеет.
    NegativeBalanceCompensated { amount: Cents },
    /// История счёта с первой записи по `through_seq` **в реальные балансы и
    /// отчётность не входит** (TRM-48).
    ///
    /// # Почему не «песочница» (переименовано 24.10)
    ///
    /// Слово «песочница» к этому моменту значило у нас **три разных вещи**:
    /// род счёта (`real · demo · sandbox`), происхождение счёта
    /// (`registration · migration · sandbox`) и — здесь — заявление о
    /// **диапазоне записей**. Три смысла одного слова это наш класс дефекта, и
    /// третий был выпущен последним, применён ни разу и потому стоил дешевле
    /// всех. Имя теперь говорит ровно то, что запись делает, и ни с чем из
    /// модели счёта пересечься не может.
    ///
    /// **Это первая и единственная запись, которая говорит о САМОЙ ЛЕНТЕ, а не о
    /// деньгах.** Денег она не трогает: свёртка её не применяет. Решение штаба от
    /// 24.10, принятое с названным условием — пропуск при свёртке делается по
    /// одному объявленному виду, а не списком исключений, который дорастёт.
    ///
    /// Форма продиктована нашими же замками: `account_events` неизменяема
    /// триггером БД, а записи сцеплены отпечатками. Удалить или переписать
    /// прошлое нельзя **физически** — разорвалась бы цепочка, то есть счёт
    /// испортился бы сильнее того, что мы исправляем. Поэтому судьба уже
    /// записанного решается **новой записью о природе старых**.
    ///
    /// `through_seq` обязателен и обязан быть **границей**: заявление без границы
    /// завтра нечем проверить, и оно тихо распространится на то, чего мы не имели
    /// в виду. Значение равно номеру предыдущей записи ленты.
    HistoryExcludedFromReporting { through_seq: u64 },
}

impl EventKind {
    /// Меняет ли запись состояние счёта.
    ///
    /// **Единственное место, где объявлен пропуск при свёртке.** Условие штаба:
    /// не «список исключений», а один названный вид; появится второй — это новое
    /// решение, а не добавление строки сюда.
    ///
    /// Написано исчерпывающим `match`, а не `matches!`, намеренно: новый вид
    /// записи **не скомпилируется**, пока кто-то не ответит здесь на вопрос
    /// «меняет ли он деньги». Умолчания у этого вопроса нет.
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
            | EventKind::DemoAccountReset { .. } => true,
            EventKind::HistoryExcludedFromReporting { .. } => false,
        }
    }
}

// ------------------------- Каноническая запись -------------------------------
//
// Один и тот же текст идёт и в отпечаток, и в хранилище: тогда «что подписано» и «что
// лежит в базе» — это буквально одно и то же, а round-trip-тест доказывает их
// совпадение без живой БД.

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

/// Разобрать запись вида `ключ=значение` в пары. Порядок не важен, лишние поля — отказ.
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

/// Поле, которого в записи могло не быть вовсе: оно появилось позже самой записи.
///
/// Отсутствие — **не порча**. Журнал неизменяем: записи, сделанные до
/// расширения формата, обязаны читаться и после него, иначе первая же правка
/// формата обнулила бы историю, то есть ровно то, ради чего журнал заведён.
/// Поэтому «нет поля» и «поле пустое» здесь одно и то же, а вот «поле есть, но
/// не разбирается» — по-прежнему порча.
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

/// Признак, появившийся позже записи. Та же терпимость, что и выше: нет поля —
/// значит, о нём тогда не знали, и это «нет», а не «испорчено».
fn flag_added_later(fs: &[(&str, &str)], key: &str) -> Result<bool, BadRecord> {
    match fs.iter().find(|(k, _)| *k == key) {
        None => Ok(false),
        Some((_, "0")) => Ok(false),
        Some((_, "1")) => Ok(true),
        Some(_) => Err(BadRecord("поле-признак не 0 и не 1")),
    }
}

/// Профиль счёта из полей записи — или профиль счетов, заведённых до TRM-44.
///
/// Отсутствие полей читается как «тогда о них не знали», и подставляется
/// **история**, а не удобство: демо, розничный, доллар, регистрация. Другого
/// рода счетов в терминале не было.
///
/// А вот поле, которое ЕСТЬ, но с неизвестным значением, — **отказ**. Разница
/// принципиальная: «не знали» и «написано непонятное» требуют разного. Второе
/// значит, что запись испорчена или пришла из будущего, и читать её как
/// демо-счёт значило бы угадывать род чужих денег.
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

/// Плечо записи об открытии, появившееся позже (TRM-53).
///
/// Нет поля — значит **1**: до TRM-53 другого плеча в терминале не было, и это
/// факт их истории, а не умолчание на всякий случай. Поле, которое есть, но не
/// читается числом или не больше нуля, — **отказ**: плечо делит нотионал, и
/// ноль здесь уронил бы расчёт, а отрицательное дало бы отрицательную маржу.
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

/// Запись не разбирается. Не «подставим умолчание», а отказ: молчаливое умолчание в
/// журнале денег — это выдуманная история.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadRecord(pub &'static str);

impl std::fmt::Display for BadRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl EventKind {
    /// Каноническая строка события: то же, что ложится в хранилище и в отпечаток.
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
        }
    }

    /// Разобрать каноническую строку обратно. Нужна хранилищу при подъёме журнала.
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
            // Старое имя принимается при РАЗБОРЕ и больше никогда не пишется.
            // Записи, сделанные под ним, сохраняют свой канон и свой отпечаток
            // (см. `AccountEvent::canon`), поэтому переименование их не трогает:
            // читаются новым видом, подписаны прежним текстом. Ради этого канон
            // и перестал пересобираться.
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
            _ => return Err(BadRecord("неизвестный вид события")),
        })
    }
}

// ============================== Запись журнала ==============================

/// 128-битный FNV-1a. Без зависимостей и детерминирован; длины хватает, чтобы
/// случайное совпадение отпечатков можно было не рассматривать.
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

/// Канонический текст источника. Строки — с префиксом длины в байтах, чтобы
/// «who=a reason=b» и «who=a reason  b» не давали одинакового отпечатка.
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

/// Запись журнала. Поля приватны: собрать её можно только через [`Journal::seal`]
/// (новая запись) или [`AccountEvent::from_storage`] (подъём уже записанной).
/// Ни одного метода, меняющего содержимое, у типа нет — «изменить запись» нечем.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountEvent {
    seq: u64,
    at_ms: i64,
    origin: Origin,
    kind: EventKind,
    /// **Каноническая строка записи — та самая, по которой считался отпечаток.**
    ///
    /// Хранится, а не выводится заново из [`Self::kind`], и это не экономия, а
    /// условие целостности. Отпечаток считается по тексту; если текст
    /// пересобирать при каждом чтении, то **любое расширение записи ломает все
    /// прежние**: старая строка разберётся (терпимость разбора у нас есть), но
    /// закодируется обратно уже с новыми полями — другим текстом и другим
    /// отпечатком. `verify` ответит `Tampered`, и счета уйдут в карантин при
    /// подъёме.
    ///
    /// Найдено при подготовке TRM-44 тестом на настоящей строке из журнала
    /// счёта. До этого мы оба — и зона, и штаб — считали `*_added_later`
    /// достаточным: он и правда делает терпимым **разбор**, но канон не
    /// сохраняет. Терпимость разбора и терпимость канона — разные свойства, и
    /// подпись держится на втором.
    ///
    /// Хранилище и так кладёт в `kind` ровно эту строку: «что подписано» и «что
    /// лежит в базе» задумывались одним и тем же. Теперь они одно и в памяти.
    canon: String,
    prev: u128,
    hash: u128,
}

impl AccountEvent {
    /// Номер записи в ленте счёта, с единицы и без пропусков.
    pub fn seq(&self) -> u64 {
        self.seq
    }
    /// Когда записано, unix-миллисекунды. Приходит снаружи: ядро детерминировано.
    pub fn at_ms(&self) -> i64 {
        self.at_ms
    }
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
    pub fn kind(&self) -> &EventKind {
        &self.kind
    }
    /// Отпечаток предыдущей записи — то, из чего складывается цепочка.
    pub fn prev(&self) -> u128 {
        self.prev
    }
    pub fn hash(&self) -> u128 {
        self.hash
    }

    /// Пересчитать собственный отпечаток из содержимого.
    fn digest(&self) -> u128 {
        // Подписывается СОХРАНЁННЫЙ текст, а не пересобранный. Разница видна
        // только на записи, чей вид изменился с момента написания, — и ровно
        // там она стоит всех счетов сразу.
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

    /// Каноническая строка записи — для хранилища и для сверки.
    pub fn canon(&self) -> &str {
        &self.canon
    }

    /// Отпечаток, посчитанный заново. Только для проверок: в бою сверкой
    /// занимается [`Journal::verify`], и пересчитывать подпись по месту незачем.
    #[cfg(test)]
    pub fn digest_for_tests(&self) -> u128 {
        self.digest()
    }

    /// Поднять запись из хранилища **как она там лежит**, включая отпечаток.
    ///
    /// Отпечаток сознательно не пересчитывается: иначе правка строки в базе молча
    /// получила бы свежий, сходящийся отпечаток, и цепочка перестала бы что-либо
    /// значить. Расхождение ловит [`Journal::verify`] при подъёме.
    /// Принимает **каноническую строку**, а не разобранный вид, и разбирает её
    /// сама.
    ///
    /// Так устроено затем, чтобы пара «текст и его разбор» не могла разойтись:
    /// вызывающему негде подставить один вид и текст от другого. Текст
    /// сохраняется как есть и дальше служит подписью — расширение записи новыми
    /// полями больше не меняет отпечаток прежних (TRM-44).
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

// ================================ Свёртка ===================================

/// Состояние счёта, восстановленное из журнала.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayState {
    /// Счёт заводился? Пустой журнал — это «счёта не было», а не «счёт с нулём».
    pub opened: bool,
    pub balance: Cents,
    pub next_id: u64,
    pub positions: Vec<Position>,
    pub pendings: Vec<PendingOrder>,
    pub closed: Vec<ClosedDeal>,
    pub model: PositionModel,
    /// События, которые свёртке не к чему применить (позиции с таким id нет).
    /// Молча пропускать их нельзя: это уже расхождение, просто обнаруженное раньше.
    pub orphans: Vec<u64>,
    /// Неизменная часть счёта, как её объявила запись о заведении (TRM-44).
    pub profile: AccountProfile,
    /// До какой записи история исключена из реальных балансов и отчётности
    /// (TRM-48). `None` — заявления не было. Это **не** состояние счёта и в
    /// сверку денег не входит: свёртка держит его, чтобы читателю было где
    /// увидеть заявление.
    pub excluded_from_reporting_through: Option<u64>,
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
        }
    }

    pub fn used_margin(&self) -> Cents {
        self.positions.iter().map(|p| p.margin).sum()
    }

    fn pos_mut(&mut self, id: u64) -> Option<&mut Position> {
        self.positions.iter_mut().find(|p| p.id == id)
    }

    /// Применить одно событие. Деньги считаются теми же функциями, что и в торговом
    /// ядре; независимо здесь воспроизводится **ветвление операции**.
    fn apply(&mut self, kind: &EventKind) {
        // Множитель валюты счёта берётся ДО изменяемых заимствований ниже:
        // позиция одалживается на правку, а валюта лежит в том же `self`.
        let minor = self.profile.currency.minor_per_unit();
        // Записи, не меняющие состояние, свёртка не применяет. Какие именно —
        // объявлено в ОДНОМ месте, `EventKind::changes_state`, и список там не
        // растёт по надобности: второй такой вид будет новым решением штаба.
        //
        // Проверка стоит здесь, до разбора, а не отдельной веткой внутри: ветка
        // «ничего не делаем» выглядела бы как забытая реализация, и следующий
        // дописал бы в неё логику.
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
            // Компенсация поднимает остаток ровно на записанную сумму. Не «в
            // ноль»: свёртка обязана получать результат из проводок, а не
            // подгонять его к ожидаемому — иначе испорченная сумма молча
            // исправится при подъёме и расхождения никто не увидит.
            EventKind::NegativeBalanceCompensated { amount } => self.balance += *amount,
            // Сброс тоже применяется ДВИЖЕНИЕМ, а не присвоением стартовой
            // суммы: свёртка обязана получать остаток из проводок. Присвоение
            // молча исправило бы испорченную запись при подъёме.
            EventKind::DemoAccountReset { adjustment } => self.balance += *adjustment,
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
                    // Маржу считает плечо ЗАПИСИ, а не сегодняшняя настройка
                    // площадки (TRM-53 шаг 2). Свёртка обязана воспроизводить
                    // прошлое таким, каким оно было: иначе смена плеча
                    // переписывала бы маржу у всех открытых позиций разом.
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
                });
            }
            EventKind::PositionIncreased {
                id,
                add_qty,
                add_entry,
                sl,
                tp,
            } => {
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
                let Some(p) = self.pos_mut(*id) else {
                    self.orphans.push(*id);
                    return;
                };
                let pnl = realized_on_close(p.side, p.entry, *exit, *qty, p.pd, p.qd, minor);
                let deal = ClosedDeal {
                    instrument: p.instrument,
                    side: p.side,
                    qty: *qty,
                    entry: p.entry,
                    exit: *exit,
                    pnl,
                    pd: p.pd,
                    qd: p.qd,
                    // Частичное сворачивание — встречная сделка, а не триггер:
                    // уровня нет, разрыв ни при чём. Значения обязаны совпасть
                    // с живым путём (`Account::merge`), иначе свёртка разойдётся
                    // со слепком и сверка покраснеет на ровном месте.
                    level: None,
                    across_gap: Some(false),
                };
                p.qty -= qty;
                let left = p.qty;
                p.margin = margin_for(p.entry, left, p.pd, p.qd, p.leverage, minor);
                self.balance += pnl;
                self.closed.push(deal);
            }
            // Свёртка считает деньги по ИСПОЛНЕННОЙ цене `exit`. `level` и
            // `across_gap` — свидетельство, а не вход расчёта: пока политику
            // исполнения не изменили, баланс обязан получаться тот же, что и
            // до появления этих полей, иначе расширение журнала молча
            // переписало бы историю счетов.
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
                    // Обе величины есть в самом событии — восстанавливаем их, а
                    // не домысливаем: свёртка обязана дать то же, что живой путь.
                    level: *level,
                    across_gap: Some(*across_gap),
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
            EventKind::PendingCancelled { id } | EventKind::PendingTriggered { id, .. } => {
                let before = self.pendings.len();
                self.pendings.retain(|p| p.id != *id);
                if self.pendings.len() == before {
                    self.orphans.push(*id);
                }
            }
            // Сюда не доходит: отсечено выше по `changes_state`. Ветка нужна
            // только ради исчерпывающего `match` — он и есть то, что заставит
            // следующего ответить, меняет ли деньги его новый вид записи.
            EventKind::HistoryExcludedFromReporting { .. } => {}
        }
    }
}

// ================================ Журнал ====================================

/// Чем сломана лента. Отличается от расхождения значений: здесь испорчена **сама
/// запись**, и доверять нельзя уже ничему после неё.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalFault {
    /// Номер не продолжает ленту: запись вставлена или потеряна.
    Sequence { expected: u64, got: u64 },
    /// Ссылка на предыдущую запись не сходится.
    Chain { seq: u64 },
    /// Содержимое не сходится с собственным отпечатком: запись изменена.
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

/// Журнал одного счёта: лента записей плюс поддерживаемая свёртка.
///
/// Свёртка хранится рядом не ради скорости отчёта, а чтобы сверка со слепком стоила
/// столько же на тысячной операции, сколько на первой: проверка, которая дорожает,
/// рано или поздно выключается.
#[derive(Debug, Clone)]
pub struct Journal {
    events: Vec<AccountEvent>,
    state: ReplayState,
    start_balance: Cents,
}

impl Journal {
    /// Плечо журналу больше не передаётся (TRM-53 шаг 2).
    ///
    /// **Свёртка не должна знать сегодняшнюю настройку площадки.** Пока
    /// знала, маржа поднятой из ленты позиции зависела от значения, которое
    /// владелец меняет в админке, и правка переписывала прошлое у всех
    /// открытых позиций разом. Плечо каждой позиции лежит в её записи об
    /// открытии; другого источника у свёртки теперь нет — не по соглашению, а
    /// потому что параметра не существует.
    pub fn new(start_balance: Cents) -> Self {
        Self {
            events: Vec::new(),
            state: ReplayState::new(start_balance),
            start_balance,
        }
    }

    /// Поднять журнал из хранилища: проверить цепочку целиком и свернуть.
    /// Испорченная лента — отказ, а не «возьмём, что разобралось».
    ///
    /// Записи кладутся как есть и проверяются **одной** [`Journal::verify`], а не
    /// по одной через [`Journal::commit`]. Разница не стилистическая: при поштучной
    /// проверке `verify` не вызывался бы ниоткуда, и снятие его мутацией не красило
    /// бы ни одного теста — проверка существовала бы на бумаге. Так и нашлось.
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
    /// Текущая свёртка — состояние, которое утверждает журнал.
    pub fn state(&self) -> &ReplayState {
        &self.state
    }

    /// Запечатать события в записи, продолжающие ленту. Журнал при этом **не меняется**:
    /// записать можно только то, что уже сохранено (см. [`Journal::commit`]).
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
                // Новая запись пишется текущим видом — он и становится её
                // каноном навсегда.
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

    /// Каким станет состояние, если эти записи принять. Журнал не трогается: сверка
    /// обязана состояться **до** того, как лента пополнится, — убрать запись нельзя.
    pub fn preview(&self, events: &[AccountEvent]) -> ReplayState {
        let mut s = self.state.clone();
        for e in events {
            s.apply(&e.kind);
        }
        s
    }

    /// Принять запись в ленту: проверить место в цепочке и свернуть.
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

    /// Пересчитать цепочку целиком: номера подряд, ссылки сходятся, отпечатки
    /// совпадают с содержимым. Это проверка **поднятой** ленты; продолжение ленты
    /// в бою проверяет [`Journal::commit`].
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

    /// Свернуть ленту с нуля, не трогая поддерживаемое состояние.
    ///
    /// Нужна сама по себе: поддерживаемая свёртка обновляется по одному событию, и
    /// если в неё закралась ошибка, сравнение её с собой этого не покажет.
    pub fn replay(&self) -> ReplayState {
        let mut s = ReplayState::new(self.start_balance);
        for e in &self.events {
            s.apply(&e.kind);
        }
        s
    }
}

// ================================ Сверка ====================================

/// Расхождение журнала со слепком. Текст — для лога и трекера.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence(pub String);

impl std::fmt::Display for Divergence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Итог сверки счёта с его журналом.
///
/// Варианты разведены сознательно: «записей нет» и «журнал недоступен» — разные беды с
/// разными действиями, и путать их нельзя. Недоступность хранилища сюда вообще не
/// попадает: это ошибка чтения, её поднимает `persistence::StoreError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalStatus {
    /// Журнал сошёлся со слепком.
    Reconciled,
    /// Журнал прочитан и пуст, а счёт — нет. Не поломка и не недоступность: история
    /// счёта отсутствует, переиграть его нечем.
    NoEntries,
    /// Лента испорчена.
    Faulted(JournalFault),
    /// Значения разошлись.
    Diverged(Vec<Divergence>),
    /// **Деньги сходятся, а происхождения у них нет** (TRM-47).
    ///
    /// Журнал непуст и считает тот же остаток, что и слепок, но в нём нет записи
    /// о заведении счёта — то есть первые деньги взялись ниоткуда.
    ///
    /// Отдельный статус, а не «расхождение», потому что это другой вопрос и
    /// другой разбор. Расхождение означает «числа не сошлись»; здесь числа
    /// сошлись, и именно поэтому прежняя сверка отвечала «в порядке». Деньги без
    /// происхождения **по сумме не ловятся**: свёртка начинается со стартового
    /// баланса, и журнал без записи о заведении даёт ровно тот же остаток, что и
    /// журнал с ней. Проверка, сравнивавшая только суммы, была обречена молчать.
    NoProvenance(Vec<Divergence>),
}

impl JournalStatus {
    pub fn ok(&self) -> bool {
        matches!(self, JournalStatus::Reconciled)
    }
    /// Короткий код для лога и выдачи наружу.
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

/// Есть ли у денег на счёте происхождение (TRM-47).
///
/// Ключевое событие жизненного цикла ровно одно: **счёт заведён**. Без него
/// журнал описывает движения денег, которые на счёте уже как-то оказались, —
/// то есть не описывает их целиком.
///
/// Вынесено отдельной функцией сознательно: это **не** сравнение величин, и
/// складывать его в одну кучу с «баланс не сошёлся» значило бы снова получить
/// один диагноз на два разных разбора. Расхождение сумм чинится поиском
/// пропавшей проводки; отсутствие происхождения не чинится вовсе — его можно
/// только зафиксировать.
///
/// `ModelAdopted` сюда НЕ входит, и это не забывчивость: его отсутствие значит
/// «модель ни разу не меняли», то есть хеджирование, — законное состояние, а не
/// пропажа. Проверять надо то, чего не может не быть.
pub fn provenance(state: &ReplayState) -> Option<Divergence> {
    (!state.opened).then(|| {
        Divergence(
            "происхождение: в журнале нет записи о заведении счёта — первые деньги взялись ниоткуда"
                .into(),
        )
    })
}

/// Сравнить восстановленное из журнала состояние со слепком счёта.
///
/// Сверяется именно то, за что отвечает журнал: деньги, то, из чего они
/// получаются, **и то, откуда они взялись** (TRM-47). Пустой вектор — сходится.
pub fn reconcile(state: &ReplayState, snap: &AccountSnapshot) -> Vec<Divergence> {
    let mut out = Vec::new();
    let mut d = |msg: String| out.push(Divergence(msg));

    // Происхождение — ПЕРВЫМ, и по порядку, а не по важности: на него смотрит
    // `audit`, когда решает, один ли это диагноз или их два.
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
    for id in &state.orphans {
        d(format!(
            "событие журнала ссылается на неизвестную позицию или ордер {id}"
        ));
    }

    // Позиции — по id, а не по порядку: в слепке они приходят из HashMap.
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
        }
    }

    out
}

/// Полная проверка счёта по его журналу: цепочка, наличие записей, сходимость.
///
/// Чинить ничего нельзя: расхождение — находка. Слепок, подогнанный под журнал (или
/// наоборот), уничтожает единственную улику того, что что-то пошло не так.
pub fn audit(journal: &Journal, snap: &AccountSnapshot) -> JournalStatus {
    if let Err(f) = journal.verify() {
        return JournalStatus::Faulted(f);
    }
    // Свёртка считается заново: сравнивать поддерживаемое состояние с самим собой
    // смысла нет, а ошибка в нём — ровно то, что ищем.
    let replayed = journal.replay();
    if journal.is_empty() {
        let fresh = replayed.balance == snap.balance
            && snap.positions.is_empty()
            && snap.pendings.is_empty()
            && snap.closed.is_empty()
            && snap.next_id == 0;
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
    // Если единственная находка — отсутствие происхождения, диагноз называется
    // своим именем. Если вдобавок разошлись числа, старший диагноз — расхождение:
    // неверные деньги срочнее неполной истории.
    if provenance(&replayed).is_some() && ds.len() == 1 {
        return JournalStatus::NoProvenance(ds);
    }
    JournalStatus::Diverged(ds)
}

// ============================ Тесты журнала =================================
//
// Журнал проверяется тем же способом, каким его будут ломать: запись меняют, запись
// убирают, событие не доходит. Каждая из трёх бед обязана иметь собственный диагноз —
// «что-то не так» в разговоре о деньгах не значит ничего.
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
        ]
    }

    // ---- Форма записи --------------------------------------------------------

    /// Канонический текст — это и то, что подписано отпечатком, и то, что ложится в
    /// базу. Если разбор потеряет хоть одно поле, история поедет молча.
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
    /// **Настоящие записи с контура поднимаются и сверяются.**
    ///
    /// Строки сняты с журнала счёта на сервере (`GET /account/journal`), а не
    /// сгенерированы текущим кодом: генератор и проверяющий не должны быть
    /// одним кодом. Отпечатки — литералы, посчитанные сторонней реализацией
    /// FNV-1a по этим же строкам.
    ///
    /// # Что эта проверка утверждала раньше и почему изменилась
    ///
    /// Прежняя редакция требовала, чтобы `decode(строка).encode()` давал ту же
    /// строку побайтно. Это было **достаточным** условием безопасности, пока
    /// отпечаток пересчитывался из разобранного вида. После того как запись
    /// стала хранить свой канон, условие перестало быть необходимым — и, хуже
    /// того, запрещало бы **любое** расширение записи навсегда.
    ///
    /// Проверяется теперь то, ради чего всё делалось: старая запись
    /// **сверяется**. Это строго сильнее — смотрит на цепочку отпечатков, а не
    /// на совпадение текста, — и не запрещает добавлять поля.
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

    /// И запись о заведении счёта — **с профилем, которого в ней не было**
    /// (TRM-44). Расширение этой записи и есть то, ради чего канон хранится:
    /// она первая в ленте каждого счёта, и её отпечаток держит всю цепочку.
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

    /// **Запись, написанная ДО расширения вида, поднимается и сверяется.**
    ///
    /// Та самая проверка, ради которой канон хранится, а не пересобирается.
    ///
    /// **Отпечаток здесь — литерал, посчитанный НЕ нашим кодом:** FNV-1a (128
    /// бит) от строки
    /// `1|1700000000000|client|position_closed id=1 exit=240053 cause=command|0…0`,
    /// посчитанный отдельной реализацией на Python. Это не придирка к чистоте —
    /// первая редакция теста брала ожидаемое значение у `digest()`, то есть у
    /// проверяемой функции, и потому **оставалась зелёной под мутацией**:
    /// подпись и проверка менялись вместе и сходились между собой. Тест
    /// утверждал «digest самосогласован» вместо «старая запись сверяется».
    ///
    /// Красит ровно одна правка: вернуть `digest` к `self.kind.encode()`. Тогда
    /// старая строка пересоберётся с новыми полями, отпечаток разойдётся с
    /// записанным, и `load` ответит `Tampered` — счёт уйдёт в карантин.
    #[test]
    fn a_record_written_before_the_shape_changed_still_verifies() {
        const OLD: &str = "position_closed id=1 exit=240053 cause=command";
        const AT: i64 = 1_700_000_000_000;
        // Посчитано сторонней реализацией FNV-1a, а не вызовом `digest`.
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

    /// Обратная сторона того же: отпечаток, посчитанный по НОВОМУ виду, к старой
    /// записи не подходит. Без этого случая предыдущий тест не доказывает, что
    /// сверка вообще умеет отказывать.
    #[test]
    fn the_new_shape_fingerprint_does_not_fit_the_old_record() {
        const OLD: &str = "position_closed id=1 exit=240053 cause=command";
        // FNV-1a от той же строки, но с `level=- gap=0` — тоже посчитан снаружи.
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

    /// **Запись об открытии, написанная до появления плеча, переигрывается с
    /// плечом 1 и даёт ту же маржу** (TRM-53, проверка ДО работы).
    ///
    /// Написана прежде, чем в запись добавлено поле плеча, и в этом её смысл:
    /// она закрепляет поведение, которое добавление обязано **сохранить**.
    /// Если после правки старая запись перестанет читаться или даст другую
    /// маржу — это увидит она, а не прод.
    ///
    /// Почему именно маржа. Записи журнала несут входные данные, а не
    /// результат: маржа пересчитывается при свёртке, и делает это **по
    /// плечу**. Значит плечо — скрытый вход свёртки, и у старых записей он
    /// обязан остаться единицей: другого в терминале не было.
    ///
    /// Строка настоящая, снята с журнала счёта на контуре.
    /// Свёртка считает маржу по плечу ЗАПИСИ (TRM-53 шаг 2).
    ///
    /// **Сегодняшней настройки свёртка больше не видит вовсе.** Параметр
    /// `lev` у `ReplayState::apply` и поле `Journal::lev` удалены: пока они
    /// были, маржа прошлого зависела от значения, которое владелец меняет
    /// сегодня. Это сильнее теста — выразить ошибку стало нечем.
    ///
    /// Тест остаётся на то, что выразить ещё можно: плечо берётся из записи,
    /// а не зашито. Прежние прогоны этого не ловили — все записи и все
    /// свёртки стояли на единице, и подмена `rec_lev` на `1` была невидима.
    #[test]
    fn the_fold_takes_the_leverage_from_the_record() {
        // 239883 × 1000 × 100 / 10^(2+4) = 23988 наименьших единиц нотионала.
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

    /// Доливка и частичное закрытие идут по плечу ПОЗИЦИИ (TRM-53 шаг 2).
    ///
    /// Позиция — единая величина от открытия до закрытия (решение штаба), и
    /// её маржа считается одним правилом. Записи о доливке своего плеча не
    /// несут — оно уже в позиции, и свёртка обязана брать его оттуда, а не
    /// подставлять единицу.
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
        // 239883 × 1000 × 100 / 10^(2+4) = 23988 наименьших единиц при плече 1.
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
        // И САМО плечо, восстановленное из записи, — единица.
        //
        // Проверяется отдельно от маржи намеренно: утверждение о марже при
        // совпадающих плечах до поля записи не дотягивается, и подмена
        // умолчания на ноль осталась бы незамеченной. Нашлось мутацией
        // «нет поля — значит 0».
        //
        // Ноль здесь не абстрактная порча: плечо делит нотионал, и ноль
        // уронил бы расчёт маржи делением на ноль в тот день, когда маржу
        // начнёт считать плечо позиции.
        assert_eq!(
            p.leverage, 1,
            "у записи без поля плечо обязано быть единицей, а не нулём и не чем-то ещё"
        );
    }

    /// **Запись под СТАРЫМ именем читается новым видом и сохраняет подпись.**
    ///
    /// Переименование `sandbox_declared` → `history_excluded_from_reporting`
    /// (24.10) меняет канон будущих записей. Прежние обязаны остаться
    /// нетронутыми: их текст — это их подпись.
    ///
    /// Ровно ради этого канон перестал пересобираться. До той починки
    /// переименование вида записи было бы невозможно в принципе — любое новое
    /// имя ломало бы отпечатки всех записей со старым.
    ///
    /// Отпечаток — литерал, посчитанный сторонней реализацией FNV-1a.
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

    /// **Пропускается при свёртке РОВНО один вид записи** (TRM-48, условие штаба).
    ///
    /// Не «список исключений, который дорастёт», а один названный вид. Этот тест
    /// и есть замок: появится второй — он покраснеет, и его придётся не
    /// дописывать, а обсуждать.
    ///
    /// `ops()` перечисляет все виды записей; `changes_state` написан исчерпывающим
    /// `match`, поэтому новый вид не скомпилируется, пока о нём не ответят, — а
    /// попав в `ops()`, он попадёт и сюда.
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

    /// Объявление песочницы **не трогает деньги**: свёртка его не применяет.
    /// Проверяется сравнением состояния до и после — всё, кроме самого заявления.
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

    /// Все причины закрытия различимы в тексте: «закрыл человек» и «сработал стоп» —
    /// это разные ответы на претензию, и склеивать их нельзя.
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

    /// Записи, сделанные ДО появления `level`/`gap`, обязаны читаться.
    ///
    /// Журнал неизменяем и лежит в основании балансов. Если расширение формата
    /// сделает вчерашние записи нечитаемыми, свёртка остановится на первой из
    /// них — то есть правка формата обнулит историю счетов. Это не
    /// снисходительность разбора: отсутствующее поле означает «тогда о нём не
    /// знали», и это ответ, а не порча.
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

    /// Терпимость к ОТСУТСТВИЮ поля — не терпимость к мусору в нём.
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

    /// Проскальзывание обязано быть ВИДНО в записи: стоп, сработавший ровно по
    /// поставленному уровню, и стоп, перепрыгнутый на разрыве, — это разные
    /// разговоры с клиентом, и в журнале они не должны выглядеть одинаково.
    #[test]
    fn slippage_is_visible_in_the_record() {
        // Рынок дошёл ровно до уровня: исполнение совпало с поставленным.
        let quiet = EventKind::PositionClosed {
            id: 1,
            exit: 5000,
            cause: CloseCause::StopLoss,
            level: Some(5000),
            across_gap: false,
        };
        // Рынок перепрыгнул уровень: поставили 5000, исполнилось по 4200.
        // Эти 800 и есть проскальзывание, и восстановить их можно только
        // потому, что записаны обе цены.
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

    // ---- Ручная проводка: форма готова до самой функции -----------------------

    /// Поля ручной проводки обязательны **по типу**: собрать запись «оператор поправил,
    /// причина не указана» нельзя. Добавлять эти поля потом значило бы переписывать
    /// журнал и все уже посчитанные отпечатки.
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

    /// Проводка оператора проходит весь путь: записывается, читается и **меняет
    /// восстановленный остаток**. Поле, которое никто не кладёт и никто не читает, —
    /// это не готовность к ADM-03, а мёртвый код.
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

    /// Источник входит в отпечаток: подменить «система» на «клиент» задним числом
    /// нельзя незаметно. Иначе ручную проводку можно было бы выдать за срабатывание
    /// движка — ровно то, что скрывает злоупотребление.
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

    // ---- Неизменяемость ------------------------------------------------------

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

    /// Правка содержимого ловится собственным отпечатком записи.
    #[test]
    fn edited_record_is_detected() {
        let j = filled();
        let mut rows: Vec<AccountEvent> = j.events().to_vec();
        let victim = &rows[1];
        rows[1] = AccountEvent::from_storage(
            victim.seq(),
            victim.at_ms(),
            victim.origin().clone(),
            &EventKind::Deposited { amount: 5_000_000 }.encode(), // было 500
            victim.prev(),
            victim.hash(), // отпечаток остался прежним
        )
        .expect("строка разбирается");
        let faulted = Journal::load(10_000_000, rows).expect_err("правка обязана ловиться");
        assert_eq!(faulted, JournalFault::Tampered { seq: 2 });
    }

    /// Правка с пересчитанным отпечатком ловится цепочкой: следующая запись ссылается
    /// на прежний. Переписать одну строку в базе и остаться незамеченным нельзя.
    #[test]
    fn edited_record_with_recomputed_fingerprint_breaks_the_chain() {
        let j = filled();
        let mut rows: Vec<AccountEvent> = j.events().to_vec();
        let victim = &rows[1];
        // Злоумышленник аккуратен: он пересчитал отпечаток изменённой записи.
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

    /// Изъятая запись ловится нумерацией. «Потерялось» и «не было» — разные истории.
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

    /// Запечатывание ленту не меняет: пока запись не сохранена, её в журнале нет.
    /// Это и даёт право сверяться ДО записи — убрать запись потом невозможно.
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

    /// Продолжение ленты проверяется и в бою, а не только при подъёме: запись,
    /// которая не продолжает ленту, в неё не попадает.
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

    /// Поддерживаемая свёртка обязана совпадать с полным пересчётом. Сравнение
    /// пересчёта с самим собой ничего не доказывает, поэтому пересчёт отдельный.
    #[test]
    fn maintained_state_matches_a_full_replay() {
        let j = filled();
        assert_eq!(j.replay(), *j.state());
    }

    // ---- Восстановление состояния и сверка ------------------------------------

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
        }
    }

    #[test]
    fn replay_of_a_matching_snapshot_has_no_divergence() {
        let j = filled();
        assert_eq!(reconcile(&j.replay(), &snap_of(&j)), vec![]);
        assert_eq!(audit(&j, &snap_of(&j)), JournalStatus::Reconciled);
    }

    /// Остаток, не подтверждённый журналом, обязан быть находкой. Это и есть вторая
    /// часть цены промедления: испорченный счёт сегодня нечем переиграть.
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

    /// Событие, которое не дошло до журнала, — самый вероятный дефект: кто-то добавит
    /// операцию и забудет её записать. Сверка обязана это показать.
    #[test]
    fn an_event_that_never_reached_the_journal_is_found() {
        let j = filled();
        let mut s = snap_of(&j);
        s.balance += 100_000; // деньги пришли, записи о них нет
        let status = audit(&j, &s);
        assert!(
            matches!(status, JournalStatus::Diverged(_)),
            "непрописанное пополнение обязано ловиться: {status}"
        );
    }

    /// «Записей нет» — отдельный диагноз, не «расхождение» и не «всё хорошо».
    /// Недоступность хранилища сюда не попадает вовсе: это ошибка чтения.
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
        };
        assert_eq!(audit(&j, &live), JournalStatus::NoEntries);
        assert_eq!(audit(&j, &live).tag(), "no_entries");

        // А пустой журнал над нетронутым счётом — это норма, а не беда.
        let fresh = AccountSnapshot {
            balance: 10_000_000,
            ..Default::default()
        };
        assert_eq!(audit(&j, &fresh), JournalStatus::Reconciled);
    }

    /// Три беды — три разных кода. Ради этого они и разведены: действия по ним разные.
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

    /// Событие о позиции, которой нет, не пропускается молча: свёртка, тихо
    /// игнорирующая непонятное, выдаёт расхождение за порядок.
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
