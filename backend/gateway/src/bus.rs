//! # bus — публикация событий счёта на шину платформы (TRM-40)
//!
//! До этого модуля публикующего **не существовало**: имени события не было в коде
//! терминала нигде, AMQP-клиента не было в зависимостях, а исходящей очередью
//! звался `Vec` в памяти брокера, исчезавший на перезапуске (Р-204). Потребитель
//! на стороне кабинета при этом построен целиком и зелен — он слушал то, чего
//! никто не слал.
//!
//! ## Три части, и каждая отвечает за свою беду
//!
//! 1. **Сборка конверта** ([`envelopes_for`]) — чистая функция над «что было» и
//!    «что стало». Ни сети, ни времени внутри: время приходит аргументом, иначе
//!    её нельзя было бы проверить.
//! 2. **Устойчивая очередь** — не здесь, а в `persistence`: таблица рядом со
//!    слепком и журналом, наполняется той же транзакцией. Событие о сделке,
//!    потерянное на перезапуске, — это деньги.
//! 3. **Релей** ([`relay_once`]) — забирает committed-строки и отправляет.
//!    **После фиксации, а не до**: опубликованное до коммита переживает откат, и
//!    наружу уходит сделка, которой не было. Отозвать её нельзя, а пропущенную
//!    публикацию чинит следующий проход.
//!
//! ## Порядок повторной доставки
//!
//! Шина может быть недоступна, и это нормальное состояние, а не авария. Строка
//! остаётся непомеченной, `attempts` растёт, `next_attempt_at` отодвигается с
//! экспоненциальным отступом (5 с → 25 с → … → час). Доставка **at-least-once**:
//! потребитель дедуплицирует по `event_id`, и это не предположение — так написан
//! его `processed_events`.
//!
//! Отказ обязан быть виден **в состоянии строки**, а не только в логе процесса.
//! У соседей ровно этот зазор оставил `attempts = 0` при циклических падениях:
//! повторы «работали» и не отработали ни разу (КОНТРАКТЫ §4а).
//!
//! ## Что здесь НЕ решается
//!
//! Без `BUS_URL` релей не запускается, но **очередь наполняется всё равно**. Это
//! не заглушка ради зелёного теста: события копятся и уедут в тот момент, когда
//! шину настроят. Молча терять их, пока адреса нет, было бы худшим из вариантов —
//! именно так выглядел массив в памяти.

use std::sync::Arc;

use broker::journal::EventKind;
use broker::{AccountSnapshot, Cents, ClosedDeal, Currency, PosSide};
use persistence::{OutboxRecord, Persistence};
use serde::Serialize;

use crate::feed::FeedInstrument;

/// Обменник платформы. Владелец — `deploy` (КОНТРАКТЫ §2); мы его только требуем.
pub const DEFAULT_EXCHANGE: &str = "platform.events";

/// `source` конверта. У продюсерской схемы платформы здесь `site-web` литералом,
/// но терминал — не витрина, и потребитель читает конверт ослабленной схемой, где
/// `source` произвольный. Это записано у него в коде словами, а не угадано нами.
pub const SOURCE: &str = "terminal";

/// Версия конверта и версия событий терминала — `v1` (реестр платформы).
pub const EVENT_VERSION: u32 = 1;

pub const EV_ACCOUNT_OPENED: &str = "terminal.account.opened";
pub const EV_BALANCE_CHANGED: &str = "terminal.balance.changed";
pub const EV_TRADE_EXECUTED: &str = "terminal.trade.executed";
/// Маржин-колл: уровень маржи дошёл до предупредительного (TRM-50).
///
/// **Имени ещё нет в контракте платформы.** Потребитель в кабинете его не
/// знает: его `TERMINAL_EVENT_SCHEMAS` перечисляет три события, и неизвестное
/// он классифицирует как `foreign` и **молча пропускает** — так у него
/// задумано, чтобы стороны выкатывались порознь.
///
/// Значит до правки на той стороне уведомление до клиента НЕ ДОЙДЁТ, а по
/// критерию владельца недошедшее предупреждение — **дефект, а не «клиент не
/// заметил»**. Это зеркало Р-204: тогда потребитель был построен, а
/// публикующего не было; здесь наоборот. Имя и состав полей согласуются со
/// штабом и site-platform, и до этого события уедут в пустоту — видимо, но
/// впустую.
pub const EV_MARGIN_CALLED: &str = "terminal.margin.called";

/// Валюта демо-счёта. Единственная: деньги терминала — центы доллара во всех
/// расчётах, и поле существует потому, что его требует схема потребителя, а не
/// потому, что валют у нас несколько. Появится вторая — появится и источник.
pub const CURRENCY: &str = "USD";

/// Сколько строк забирает один проход релея.
pub const BATCH: i64 = 20;
/// Аренда взятой строки: столько времени её не возьмёт второй экземпляр.
pub const LEASE_MS: i64 = 30_000;
/// Пауза между проходами, когда очередь пуста.
pub const POLL_MS: u64 = 1_000;

/// Отступ перед следующей попыткой: 5 с, 25 с, 125 с, … не больше часа.
///
/// Сетка та же, что у релея платформы, и это не совпадение: два разных закона
/// повторов в одном контуре означают, что при общем отказе шины восстановление
/// идёт двумя разными волнами, и предсказать нагрузку нельзя ни по одной.
pub fn next_delay_ms(attempts: i32) -> i64 {
    const BASE: i64 = 5_000;
    const MAX: i64 = 3_600_000;
    let steps = attempts.max(1) - 1;
    let mut delay = BASE;
    for _ in 0..steps {
        delay = delay.saturating_mul(5);
        if delay >= MAX {
            return MAX;
        }
    }
    delay.min(MAX)
}

// ============================ Конверт =======================================

/// Конверт событий платформы, v1.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Envelope {
    pub event_id: String,
    /// Ключ маршрутизации и имя события — одно и то же поле, так у платформы.
    pub event: String,
    pub version: u32,
    pub occurred_at: String,
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub data: serde_json::Value,
}

impl Envelope {
    pub fn to_record(&self) -> OutboxRecord {
        OutboxRecord {
            event_id: self.event_id.clone(),
            routing_key: self.event.clone(),
            // `unwrap` недостижим: все поля конверта — строки, числа и уже собранный
            // `Value`. Падать здесь нечему, а возвращать `Result` ради недостижимой
            // ветки значило бы просить вызывающего обработать то, чего не бывает.
            payload: serde_json::to_string(self).expect("конверт сериализуем"),
        }
    }
}

/// `event_id`, **выведенный из факта**, а не из случайности.
///
/// Это не оптимизация, а свойство: одно и то же событие, записанное дважды
/// (повторный проход, восстановление, гонка двух путей), даёт **один и тот же**
/// идентификатор — и в очереди его отсечёт `ON CONFLICT`, а у потребителя
/// `processed_events`. Со случайным `uuid4` тот же факт уехал бы двумя разными
/// событиями, и дедуп потребителя не помог бы: он дедуплицирует по `event_id`.
///
/// Namespace — URL, имя — `urn:` с нашим пространством. Пятая версия (SHA-1 от
/// имени), поэтому значение воспроизводимо и проверяется тестом по константе.
pub fn event_id(kind: &str, key: &str) -> String {
    let name = format!("urn:terminal:{kind}:{key}");
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, name.as_bytes()).to_string()
}

/// Момент ISO-8601 с миллисекундами и `Z`. Схема потребителя — `z.string().datetime()`.
pub fn iso_millis(at_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at_ms)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp_millis(0).expect("эпоха представима"))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Целое в единицах точности → десятичный текст без потерь.
///
/// Внутри терминала цены и объёмы — целые в единицах точности инструмента
/// (красная линия №1). На проводе схема потребителя объявляет `number`, и
/// отправленное «как есть» показало бы цену в 10^d раз больше — ровно тот отказ,
/// про который написано в КОНТРАКТАХ §5 для ручки истории. Поэтому преобразование
/// делает **публикующий**, и делает его текстом, а не делением в `f64`: текст
/// точен по построению.
pub fn decimal(raw: i64, decimals: u8) -> String {
    if decimals == 0 {
        return raw.to_string();
    }
    let neg = raw < 0;
    let abs = (raw as i128).unsigned_abs();
    let p = 10u128.pow(decimals as u32);
    let (int, frac) = (abs / p, abs % p);
    let width = decimals as usize;
    let body = format!("{int}.{frac:0width$}");
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// Десятичный текст → JSON-число ровно с теми цифрами, что в тексте.
fn num(dec: &str) -> serde_json::Value {
    serde_json::from_str(dec).unwrap_or(serde_json::Value::Null)
}

/// Денежная величина на проводе — **тройкой** (Р-293, шаг 1).
///
/// # Почему тройкой, а не переименованием
///
/// `balanceCents` для счёта не в долларах врёт. Но `balanceMinor` оставил бы
/// получателя там же, где были мы до `TRM-55`: **целое без единицы**. Угадать
/// точность он может только одним способом — «наверное, сотые», — и мы бы
/// перенесли дефект через границу вместо того, чтобы его закрыть.
///
/// Поэтому сумма, код валюты и число знаков стоят **вместе**: порознь они
/// бессмысленны.
///
/// # Порядок перехода, и сокращать его нельзя
///
/// Шаг 1 (здесь): шлём **и старое поле, и тройку**, с тем же числом. Лишнее
/// поле потребитель срезает молча — это безопасно, в отличие от нового
/// значения перечня.
/// Шаги 2–3 у потребителя: принять необязательной, затем обязательной.
/// Шаг 4: перестать слать старое поле. **Снятое раньше четвёртого шага
/// уронит разбор, и вместе с суммой потеряется всё событие.**
///
/// Ловушка названа заранее: пока валюта одна, числа совпадают и шаги кажутся
/// лишними. Расхождение появится ровно тогда, когда заведут счёт в другой
/// валюте, — то есть когда проверять уже никто не будет.
fn money(data: &mut serde_json::Value, prefix: &str, amount: Cents, currency: Currency) {
    data[format!("{prefix}Minor")] = cents(amount);
    data["currency"] = serde_json::Value::from(currency.code());
    data["currencyMinorUnits"] = serde_json::Value::from(currency.minor_units());
}

/// Центы → JSON-число. Деньги целые и остаются целыми.
fn cents(v: i128) -> serde_json::Value {
    // За i64 центы не выйдут: это $92 квадриллиона. Насыщение вместо паники —
    // событие о деньгах не повод ронять торговлю, а значение на пределе само себя
    // объявит нелепым в проекции.
    serde_json::Value::from(v.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
}

/// Сторона сделки на проводе.
///
/// Событие описывает **сделку**, а не закрывающую ногу: позиция была длинной —
/// значит `buy`. Обратное отображение («закрытие лонга — это sell») показало бы
/// в ленте кабинета сторону, противоположную той, что человек видел в терминале.
fn side_tag(s: PosSide) -> &'static str {
    match s {
        PosSide::Long => "buy",
        PosSide::Short => "sell",
    }
}

/// Символ в написании платформы (`BTCUSD`), а не внутренний id и не `ETH-USDT`.
///
/// Нумерация инструментов — наша деталь и за границу не выходит (Р-024). Написание
/// то же, которым платформа задаёт allow-list: иначе у одного инструмента было бы
/// два имени на одной границе.
pub fn platform_symbol(f: &FeedInstrument) -> String {
    let up: String = f
        .binance
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    match up.strip_suffix("USDT") {
        Some(base) if !base.is_empty() => format!("{base}USD"),
        _ => up,
    }
}

// ========================= Сборка событий ===================================

/// Чем счёт был до операции — ровно те две величины, по которым видно изменение.
///
/// Отдельным типом, а не парой: пара `(usize, i128)` на вызове читается как
/// «сколько-то и сколько-то», и перепутать её половины ничто не мешает.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Before {
    /// Сколько сделок уже лежало в истории счёта.
    pub closed: usize,
    /// Каким был остаток.
    pub balance: Cents,
}

/// Всё, что публикующему нужно знать помимо самого изменения счёта.
pub struct Addressee<'a> {
    /// Слаг сайта из провалидированного токена.
    pub tenant: &'a str,
    /// Внешний идентификатор пользователя сайта (`sub` токена).
    pub sub: &'a str,
    /// Внутренний номер счёта — только для ключа идентификатора события.
    pub user: u64,
    /// Номер последней записи журнала этой операции.
    ///
    /// Нужен как ключ идемпотентности там, где самого изменения для этого мало:
    /// два перевода на одну и ту же сумму — разные события, а час и остаток у них
    /// могут совпасть. Номер в ленте не совпадает никогда: лента только
    /// дописывается, и повтор номера у нас — отказ записи.
    pub seq: u64,
    /// Момент запечатывания записей журнала.
    pub at_ms: i64,
}

/// Собрать конверты по изменению счёта. Чистая функция: сеть, время и хранилище —
/// снаружи.
///
/// `before` — то, чем счёт был **по журналу** до этой операции, `after` — слепок
/// после, `kinds` — что сказал об этом сам брокер. Берём всё три намеренно: в
/// слепке есть величины (объём, цена входа, P&L), в записях журнала — смысл (счёт
/// заведён, деньги внесены), и ни одного из них по отдельности не хватает.
///
/// Почему «до» приходит свёрткой журнала, а не вторым слепком. У ручных операций
/// слепок «до» есть — его снимают для отката. У сработавшего стопа его нет вовсе:
/// монитор узнаёт о затронутых счетах **после** мутации. Снимать слепки всех
/// счетов каждые 500 мс ради одного сработавшего — цена, которую платили бы все.
/// Журнал же хранит свёртку постоянно и обновляет её по одной записи.
///
/// `instrument` отдаёт инструмент по внутреннему id; `None` — инструмента нет в
/// справочнике, и тогда события по нему **не будет**: выдуманный символ в ленте
/// кабинета хуже отсутствующей строки, потому что его не отличить от настоящего.
pub fn envelopes_for(
    who: &Addressee<'_>,
    before: &Before,
    after: &AccountSnapshot,
    kinds: &[EventKind],
    instrument: &dyn Fn(u32) -> Option<FeedInstrument>,
) -> Vec<Envelope> {
    let mut out = Vec::new();
    let occurred_at = iso_millis(who.at_ms);

    // --- terminal.account.opened --------------------------------------------
    // Повод — запись журнала, а не «счёта не было в слепке»: заведение счёта это
    // факт, который брокер объявляет сам, и читать его по косвенным признакам
    // значило бы завести второй источник правды о том же.
    if let Some(EventKind::AccountOpened { balance, .. }) = kinds
        .iter()
        .find(|k| matches!(k, EventKind::AccountOpened { .. }))
    {
        let mut data = serde_json::json!({
            "tenant": who.tenant,
            "userId": who.sub,
            "currency": after.profile.currency.code(),
            "balanceCents": cents(*balance),
        });
        money(&mut data, "balance", *balance, after.profile.currency);
        out.push(Envelope {
            event_id: event_id(EV_ACCOUNT_OPENED, &format!("{}:{}", who.tenant, who.sub)),
            event: EV_ACCOUNT_OPENED.to_string(),
            version: EVENT_VERSION,
            occurred_at: occurred_at.clone(),
            source: SOURCE,
            subject: Some(who.sub.to_string()),
            data,
        });
    }

    // --- terminal.trade.executed --------------------------------------------
    // Источник — ХВОСТ истории закрытых сделок, а не `PositionClosed` из журнала:
    // в записи журнала лежат входные данные операции (красная линия сверки), а
    // объём, цена входа и реализованный P&L считаются при свёртке. Брать их из
    // записи значило бы досочинять.
    for (idx, deal) in after.closed.iter().enumerate().skip(before.closed) {
        let Some(f) = instrument(deal.instrument) else {
            eprintln!(
                "[bus] сделка по неизвестному инструменту {} — события не будет, символ выдумывать нельзя",
                deal.instrument
            );
            continue;
        };
        out.push(trade_executed(
            who,
            &occurred_at,
            idx,
            deal,
            &f,
            after.profile.currency,
        ));
    }

    // --- terminal.balance.changed -------------------------------------------
    // Повод — изменившийся остаток, а не «была сделка»: пополнение и ручная
    // проводка меняют деньги без сделки, а частичное сворачивание — со сделкой.
    // Признак «деньги стали другими» покрывает оба и не знает про их причины.
    if after.balance != before.balance {
        let reason = balance_reason(kinds, before, after);
        let mut data = serde_json::json!({
            "tenant": who.tenant,
            "userId": who.sub,
            "balanceCents": cents(after.balance),
            "positions": positions_json(after, instrument),
        });
        money(&mut data, "balance", after.balance, after.profile.currency);
        if let Some(r) = reason {
            // Предохранитель, а не вежливость: значение, которого нет в перечне
            // потребителя, роняет разбор ВСЕГО события, и остаток счёта в
            // кабинете не обновится. Лучше уехать без причины, чем без денег.
            match readable_reason(r) {
                Some(ok) => data["reason"] = serde_json::Value::from(ok),
                None => eprintln!(
                    "[bus] причина «{r}» потребителю неизвестна — событие уедет без неё, иначе потерялся бы и остаток"
                ),
            }
        }
        out.push(Envelope {
            // Ключ — счёт и номер записи журнала: повторная публикация той же
            // операции даёт тот же идентификатор, а два разных перевода на одну
            // сумму остаются двумя событиями.
            event_id: event_id(EV_BALANCE_CHANGED, &format!("{}:{}", who.user, who.seq)),
            event: EV_BALANCE_CHANGED.to_string(),
            version: EVENT_VERSION,
            occurred_at,
            source: SOURCE,
            subject: Some(who.sub.to_string()),
            data,
        });
    }

    out
}

/// Причины изменения остатка, которые потребитель **умеет прочитать**.
///
/// Копия закрытого перечня из его схемы
/// (`packages/api-client/src/schemas/terminal-events.ts`,
/// `reason: z.enum(['trade', 'deposit', 'reset', 'adjustment'])`). Копия, а не
/// ссылка: репозитории разные, общего дерева у них нет — ровно как у эталонной
/// таблицы юрисдикций.
///
/// # Почему это не формальность, а предохранитель
///
/// Границa ведёт себя **несимметрично**, и это неочевидно:
///
/// - **незнакомое ПОЛЕ** потребитель срезает и не замечает — лишние ключи
///   `z.object` отбрасывает молча, так у него и написано;
/// - **незнакомое ЗНАЧЕНИЕ перечня** роняет разбор **всего события**. Оно
///   становится `malformed`, уезжает в разбор, и вместе с причиной теряется
///   **остаток счёта**, который в этом событии и ехал.
///
/// То есть добавить поле можно в одиночку, а значение причины — нельзя: цена
/// ошибки не «причину не покажут», а «деньги в кабинете не обновятся».
pub const CONSUMER_KNOWN_REASONS: [&str; 4] = ["trade", "deposit", "reset", "adjustment"];

/// Причина, пригодная к отправке, или `None` — если потребитель её не знает.
///
/// Отдельной функцией, а не веткой по месту, **чтобы её можно было вызвать с
/// любым значением**. Проверка, идущая через сборку конверта, до неё не
/// дотягивается: `balance_reason` возвращает только значения из перечня.
pub fn readable_reason(r: &str) -> Option<&str> {
    CONSUMER_KNOWN_REASONS.contains(&r).then_some(r)
}

/// Причина изменения остатка — только из того, что брокер сказал сам.
///
/// Словарь причин у потребителя закрытый (`trade` · `deposit` · `reset` ·
/// `adjustment`), поле **опционально**, и в сомнительном случае мы его не
/// кладём. Пустое поле читается как «причина не названа»; подставленное наугад
/// `adjustment` читалось бы как «ручная проводка» — то есть утверждало бы о
/// деньгах то, чего мы не знаем.
fn balance_reason(
    kinds: &[EventKind],
    before: &Before,
    after: &AccountSnapshot,
) -> Option<&'static str> {
    if after.closed.len() > before.closed {
        return Some("trade");
    }
    if kinds
        .iter()
        .any(|k| matches!(k, EventKind::Deposited { .. }))
    {
        return Some("deposit");
    }
    // Сброс демо-счёта (TRM-54). `reset` уже есть в перечне потребителя, так
    // что причина доедет и ничего не сломает.
    if kinds
        .iter()
        .any(|k| matches!(k, EventKind::DemoAccountReset { .. }))
    {
        return Some("reset");
    }
    None
}

/// Снимок открытых позиций для `balance.changed`. Потребитель заменяет свой набор
/// целиком, поэтому отдаём именно снимок, а не изменения.
fn positions_json(
    snap: &AccountSnapshot,
    instrument: &dyn Fn(u32) -> Option<FeedInstrument>,
) -> serde_json::Value {
    let mut items = Vec::new();
    for p in &snap.positions {
        let Some(f) = instrument(p.instrument) else {
            eprintln!(
                "[bus] позиция по неизвестному инструменту {} — в снимок не попадёт",
                p.instrument
            );
            continue;
        };
        items.push(serde_json::json!({
            "symbol": platform_symbol(&f),
            "side": side_tag(p.side),
            "volume": num(&decimal(p.qty, p.qd)),
            "entryPrice": num(&decimal(p.entry, p.pd)),
        }));
    }
    serde_json::Value::Array(items)
}

/// Конверт `terminal.trade.executed`.
///
/// **Уровень и признак разрыва едут опциональными** — решение штаба Р-193, и это
/// не послабление, а требование: у рыночного закрытия уровня нет вовсе, и строгое
/// поле обнулило бы разбор событий, уже лежащих в очередях и проекциях. Терминал
/// наступил ровно на это у себя в журнале и починил терпимостью к отсутствию поля.
///
/// **`across_gap` внутри, `acrossGap` на шине** — кодировка на границе, записанная
/// в КОНТРАКТАХ §5, и преобразование делает публикующий. Каждая сторона,
/// переводящая сама, — это уже второе имя.
///
/// `level` трёхзначен внутри (`Some(true)`/`Some(false)`/`None`) и остаётся
/// трёхзначным снаружи: **отсутствие поля — это «неизвестно»**, а не «нет».
/// Сделки, закрытые до появления признака, о нём не знают, и `false` утверждал бы
/// за них то, чего мы не видели.
fn trade_executed(
    who: &Addressee<'_>,
    occurred_at: &str,
    idx: usize,
    deal: &ClosedDeal,
    f: &FeedInstrument,
    currency: Currency,
) -> Envelope {
    // `price` — цена ИСПОЛНЕНИЯ (`exit`). Переименований нет: у потребителя это
    // поле уже значит исполнение, у нас зовётся иначе и означает то же (Р-193).
    let mut data = serde_json::json!({
        "tenant": who.tenant,
        "userId": who.sub,
        // Номер сделки в истории счёта — он же её адрес в `GET /deals/closed`.
        // Устойчив между перезапусками: история только дописывается.
        "tradeId": format!("{}:{}", who.user, idx),
        "symbol": platform_symbol(f),
        "side": side_tag(deal.side),
        "volume": num(&decimal(deal.qty, deal.qd)),
        "price": num(&decimal(deal.exit, deal.pd)),
        "executedAt": occurred_at,
        "realizedPnlCents": cents(deal.pnl),
    });
    money(&mut data, "realizedPnl", deal.pnl, currency);
    if let Some(level) = deal.level {
        data["level"] = num(&decimal(level, deal.pd));
    }
    if let Some(gap) = deal.across_gap {
        data["acrossGap"] = serde_json::Value::from(gap);
    }
    Envelope {
        // Ключ — счёт и номер сделки в его истории. Та же сделка, отданная на
        // публикацию дважды, даёт один `event_id` и один раз доедет.
        event_id: event_id(EV_TRADE_EXECUTED, &format!("{}:{}", who.user, idx)),
        event: EV_TRADE_EXECUTED.to_string(),
        version: EVENT_VERSION,
        occurred_at: occurred_at.to_string(),
        source: SOURCE,
        subject: Some(format!("{}:{}", who.user, idx)),
        data,
    }
}

/// Конверт `terminal.margin.called` (TRM-50).
///
/// Несёт **оба уровня** — достигнутый и тот, на котором закрывают. Одного мало:
/// «ваш уровень 97 %» ничего не говорит человеку, который не помнит, где порог,
/// а предупреждение, из которого не видно, сколько осталось, не помогает
/// решить, вносить деньги или закрывать позицию.
///
/// Суммы — целые центы, как и везде в деньгах. Уровни — целые проценты: дробная
/// часть уровня маржи не несёт смысла, по которому человек принимает решение.
pub fn margin_called(
    who: &Addressee<'_>,
    level_pct: i128,
    stop_out_pct: Option<i128>,
    equity: Cents,
    used_margin: Cents,
) -> Envelope {
    let mut data = serde_json::json!({
        "tenant": who.tenant,
        "userId": who.sub,
        "marginLevelPct": cents(level_pct),
        "equityCents": cents(equity),
        "usedMarginCents": cents(used_margin),
    });
    if let Some(so) = stop_out_pct {
        data["stopOutLevelPct"] = cents(so);
    }
    Envelope {
        // Ключ — счёт и момент. Защёлка на стороне брокера уже не даёт
        // повторяться, пока счёт не выбрался; одинаковый идентификатор здесь
        // склеил бы ДВА РАЗНЫХ попадания в опасность в одно событие.
        event_id: event_id(EV_MARGIN_CALLED, &format!("{}:{}", who.user, who.at_ms)),
        event: EV_MARGIN_CALLED.to_string(),
        version: EVENT_VERSION,
        occurred_at: iso_millis(who.at_ms),
        source: SOURCE,
        subject: Some(who.sub.to_string()),
        data,
    }
}

// ============================ Отправка ======================================

#[derive(Debug, Clone)]
pub struct BusError(pub String);

impl std::fmt::Display for BusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for BusError {}

/// Отправитель в шину. Трейт — чтобы повторную доставку можно было проверить на
/// отказывающем отправителе, а не на отключённой шине.
/// `double_must_use` глушится точечно по той же причине, что у `Persistence`:
/// атрибут ставит не наш код, а раскрытие `#[async_trait]`.
#[allow(clippy::double_must_use)]
#[async_trait::async_trait]
pub trait BusPublisher: Send + Sync {
    /// Отправить один конверт. `event_id` уходит `message_id`-ом сообщения: по нему
    /// дедуплицирует потребитель, и он обязан совпадать с полем внутри конверта.
    async fn publish(
        &self,
        routing_key: &str,
        event_id: &str,
        payload: &str,
    ) -> Result<(), BusError>;
}

/// Один проход релея. Возвращает число взятых строк.
///
/// Разделение «проход» / «цикл» не стилистическое: отказ публикации обязан быть
/// виден в состоянии строки, а проверить это можно только на связке «отправитель
/// отказал → что лежит в очереди». Внутри бесконечного цикла такой проверки не
/// существует — у соседей она ровно в этом зазоре и не существовала.
pub async fn relay_once(
    store: &dyn Persistence,
    publisher: &dyn BusPublisher,
    batch: i64,
) -> Result<usize, persistence::StoreError> {
    let claimed = store.claim_outbox(batch, LEASE_MS).await?;
    let mut sent = 0usize;
    for row in &claimed {
        match publisher
            .publish(&row.routing_key, &row.event_id, &row.payload)
            .await
        {
            Ok(()) => {
                store.mark_published(row.id).await?;
                sent += 1;
            }
            Err(e) => {
                let attempts = row.attempts + 1;
                let delay = next_delay_ms(attempts);
                eprintln!(
                    "[bus] отправка не удалась {} event_id={} попытка={attempts} повтор через {delay} мс: {e}",
                    row.routing_key, row.event_id
                );
                store.mark_failed(row.id, attempts, delay, &e.0).await?;
                // Дальше по пачке не идём: шина либо лежит, либо нет, и остальные
                // строки отказали бы так же — но с инкрементом `attempts`, который
                // увёл бы их отступ в часы из-за одной чужой неудачи.
                break;
            }
        }
    }
    Ok(sent)
}

/// Фоновый релей. Запускается только когда шина настроена: без адреса отправлять
/// некуда, и делать вид, что отправляем, нельзя.
pub fn spawn_relay(store: Arc<dyn Persistence>, publisher: Arc<dyn BusPublisher>) {
    tokio::spawn(async move {
        loop {
            match relay_once(store.as_ref(), publisher.as_ref(), BATCH).await {
                // Пачка полная — очередь длиннее одной пачки, идём сразу дальше.
                Ok(n) if n as i64 >= BATCH => continue,
                Ok(_) => {}
                Err(e) => eprintln!("[bus] исходящая очередь недоступна: {e}"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
        }
    });
}

// ----------------------------- AMQP -----------------------------------------

/// Клиент шины на lapin. Соединение ленивое и пересоздаётся после разрыва:
/// держать мёртвый канал в поле — значит отказывать навсегда с посторонней
/// ошибкой вместо настоящей (урок релея платформы, REL-04).
pub struct AmqpPublisher {
    url: String,
    exchange: String,
    /// Соединение и канал вместе: lapin закрывает соединение при `drop`, и канал,
    /// переживший своё соединение, отказывал бы посторонней ошибкой вместо
    /// настоящей. Поэтому они живут и умирают одним полем.
    link: tokio::sync::Mutex<Option<(lapin::Connection, lapin::Channel)>>,
}

impl AmqpPublisher {
    pub fn new(url: String, exchange: String) -> Self {
        Self {
            url,
            exchange,
            link: tokio::sync::Mutex::new(None),
        }
    }

    async fn channel(&self) -> Result<lapin::Channel, BusError> {
        let mut slot = self.link.lock().await;
        if let Some((conn, ch)) = slot.as_ref() {
            if conn.status().connected() && ch.status().connected() {
                return Ok(ch.clone());
            }
            *slot = None;
        }
        let conn = lapin::Connection::connect(&self.url, lapin::ConnectionProperties::default())
            .await
            .map_err(|e| BusError(format!("соединение с шиной: {e}")))?;
        let ch = conn
            .create_channel()
            .await
            .map_err(|e| BusError(format!("канал: {e}")))?;
        // ПАССИВНОЕ объявление обменника (КОНТРАКТЫ §4а, целевое состояние; у
        // релея и консюмера платформы так же). Два следствия, и второе важнее:
        //  1. `exchange.declare` требует право `configure` ВСЕГДА, даже когда
        //     обменник уже есть, — с активным объявлением честный publish-only
        //     кредл невозможен;
        //  2. пропавший обменник становится немедленным отказом вместо тихого
        //     пересоздания. Активное объявление завело бы его заново, мы бы
        //     публиковали в пустоту и выглядели исправными.
        ch.exchange_declare(
            &self.exchange,
            lapin::ExchangeKind::Topic,
            lapin::options::ExchangeDeclareOptions {
                passive: true,
                durable: true,
                ..Default::default()
            },
            lapin::types::FieldTable::default(),
        )
        .await
        .map_err(|e| BusError(format!("обменник {}: {e}", self.exchange)))?;
        // Режим подтверждений. Без него ожидание подтверждения завершается
        // мгновенно и означает «брокер его не запрашивал» — то есть «успех»
        // публикации был бы «записано в сокет», а строка пометилась бы
        // отправленной при недошедшем событии. Включаем ДО первой публикации:
        // канал, переведённый в режим позже, не подтвердит уже ушедшее.
        ch.confirm_select(lapin::options::ConfirmSelectOptions::default())
            .await
            .map_err(|e| BusError(format!("режим подтверждений: {e}")))?;
        // В поле — только после того, как всё удалось. Иначе мёртвый канал осел бы
        // в кеше и следующая публикация падала бы не на той ошибке.
        *slot = Some((conn, ch.clone()));
        Ok(ch)
    }
}

#[async_trait::async_trait]
impl BusPublisher for AmqpPublisher {
    async fn publish(
        &self,
        routing_key: &str,
        event_id: &str,
        payload: &str,
    ) -> Result<(), BusError> {
        let ch = self.channel().await?;
        let props = lapin::BasicProperties::default()
            // `persistent`: сообщение переживает перезапуск брокера. Без этого
            // устойчивая очередь у нас была бы, а у шины — нет, и событие
            // терялось бы на последнем шаге.
            .with_delivery_mode(2)
            .with_content_type("application/json".into())
            .with_message_id(event_id.into());
        let confirm = ch
            .basic_publish(
                &self.exchange,
                routing_key,
                lapin::options::BasicPublishOptions::default(),
                payload.as_bytes(),
                props,
            )
            .await
            .map_err(|e| BusError(format!("публикация: {e}")))?
            // Ждём подтверждения брокера. Без ожидания «успех» означал бы
            // «записано в сокет» — строка пометилась бы отправленной, а событие
            // могло не дойти. Это ровно тот отказ, от которого заведена очередь.
            .await
            .map_err(|e| BusError(format!("подтверждение: {e}")))?;
        if confirm.is_nack() {
            return Err(BusError("брокер не подтвердил сообщение (nack)".into()));
        }
        // `NotRequested` означало бы, что подтверждений у канала не просили —
        // то есть `confirm_select` до публикации не выполнился. Это не успех:
        // ровно в этом случае «отправлено» значит «записано в сокет».
        if matches!(
            confirm,
            lapin::publisher_confirm::Confirmation::NotRequested
        ) {
            return Err(BusError(
                "канал не в режиме подтверждений — отправку нечем подтвердить".into(),
            ));
        }
        Ok(())
    }
}

/// Собрать отправителя из окружения. `None` — шина не настроена.
pub fn publisher_from_env() -> Option<Arc<dyn BusPublisher>> {
    let url = std::env::var("BUS_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())?;
    let exchange = std::env::var("BUS_EXCHANGE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_EXCHANGE.to_string());
    println!("[bus] публикация включена: обменник {exchange}");
    Some(Arc::new(AmqpPublisher::new(url, exchange)))
}

// ============================== Проверки ====================================
//
// Что здесь проверяется и почему именно это.
//
// Форма события — межсистемный контракт: потребитель построен, зелен и слушает
// (КОНТРАКТЫ §5). Ошибка в имени поля или в разрядности цены не красит ни одну
// сторону — обе остаются зелёными, а в кабинете появляется неверное число. Поэтому
// проверки идут по ЗНАЧЕНИЯМ на проводе, а не по тому, что функция «что-то вернула».

#[cfg(test)]
mod tests {
    use super::*;
    use broker::journal::{CloseCause, EventKind};
    use broker::{AccountSnapshot, ClosedDeal, PosSide, Position, PositionModel};

    const TENANT: &str = "apex-ru";
    const SUB: &str = "user-42";

    fn btc() -> FeedInstrument {
        // BTCUSDT: цена с двумя знаками, объём с пятью.
        FeedInstrument {
            id: 1,
            symbol: "BTC-USDT".into(),
            binance: "BTCUSDT".into(),
            base: 1,
            quote: 100,
            price_decimals: 2,
            qty_decimals: 5,
        }
    }

    fn known(id: u32) -> Option<FeedInstrument> {
        (id == 1).then(btc)
    }

    fn empty_snap() -> AccountSnapshot {
        AccountSnapshot {
            profile: Default::default(),
            balance: 1_000_000,
            next_id: 1,
            positions: Vec::new(),
            pendings: Vec::new(),
            closed: Vec::new(),
            model: PositionModel::Hedging,
        }
    }

    fn deal(level: Option<i64>, gap: Option<bool>) -> ClosedDeal {
        ClosedDeal {
            instrument: 1,
            side: PosSide::Long,
            qty: 150_000,     // 1.50000 при qd = 5
            entry: 6_700_000, // 67 000.00 при pd = 2
            exit: 6_823_412,  // 68 234.12
            pnl: 185_118,
            pd: 2,
            qd: 5,
            level,
            across_gap: gap,
        }
    }

    fn who(seq: u64) -> Addressee<'static> {
        Addressee {
            tenant: TENANT,
            sub: SUB,
            user: 7,
            seq,
            at_ms: 1_767_225_600_123,
        }
    }

    fn after_one_deal(level: Option<i64>, gap: Option<bool>) -> AccountSnapshot {
        let mut s = empty_snap();
        s.balance = 1_185_118;
        s.closed = vec![deal(level, gap)];
        s
    }

    fn before_empty() -> Before {
        Before {
            closed: 0,
            balance: 1_000_000,
        }
    }

    fn trade_of(evs: &[Envelope]) -> &Envelope {
        evs.iter()
            .find(|e| e.event == EV_TRADE_EXECUTED)
            .expect("событие о сделке обязано быть")
    }

    // ---- Цена и объём на проводе ------------------------------------------

    /// Цена уезжает ДЕСЯТИЧНОЙ, а не целой в единицах точности инструмента.
    ///
    /// Схема потребителя объявляет `price` числом и кладёт его в уведомление как
    /// есть. Целое `6823412` прошло бы её проверку и показало бы человеку цену в
    /// сто раз больше — обе стороны при этом зелёные. Этот отказ назван в
    /// КОНТРАКТАХ §5 для ручки истории; на шине он тот же, и преобразование здесь
    /// делает публикующий.
    #[test]
    fn price_and_volume_travel_as_decimals() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, Some(false)),
            &[],
            &known,
        );
        let d = &trade_of(&evs).data;
        assert_eq!(
            d["price"],
            serde_json::json!(68_234.12),
            "цена в валюте, не в единицах точности"
        );
        assert_eq!(
            d["volume"],
            serde_json::json!(1.5),
            "объём в лотах, не в единицах точности"
        );
        assert_eq!(
            d["realizedPnlCents"],
            serde_json::json!(185_118),
            "деньги остаются целыми центами — их схема так и объявляет"
        );
    }

    /// Преобразование точное: текстом, а не делением с накоплением ошибки.
    #[test]
    fn decimal_is_exact_at_the_edges() {
        assert_eq!(decimal(6_823_412, 2), "68234.12");
        assert_eq!(decimal(1, 8), "0.00000001");
        assert_eq!(decimal(-1, 2), "-0.01");
        assert_eq!(decimal(0, 5), "0.00000");
        assert_eq!(decimal(123, 0), "123", "ноль знаков — целое остаётся целым");
        assert_eq!(
            decimal(i64::MAX, 2),
            "92233720368547758.07",
            "предел не переполняет"
        );
    }

    // ---- Уровень и признак разрыва (Р-193, КОНТРАКТЫ §5) -------------------

    /// Имя на шине — `acrossGap`; внутри терминала `across_gap`. Кодировку делает
    /// публикующий, и это записано в контракте. Вторая сторона, переводящая сама,
    /// — уже второе имя.
    #[test]
    fn gap_flag_is_camel_case_on_the_wire() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(Some(6_800_000), Some(true)),
            &[],
            &known,
        );
        let d = &trade_of(&evs).data;
        assert_eq!(d["acrossGap"], serde_json::json!(true));
        assert!(
            d.get("across_gap").is_none(),
            "внутреннее имя за границу не выходит"
        );
        assert_eq!(
            d["level"],
            serde_json::json!(68_000.0),
            "уровень — в тех же единицах, что цена"
        );
    }

    /// **Отсутствие поля означает «неизвестно», а не «нет».**
    ///
    /// У рыночного закрытия уровня нет вовсе, а про сделки, закрытые до появления
    /// признака разрыва, мы не знаем. `false` утверждал бы за них то, чего мы не
    /// видели: отсутствующее значение ищут, лгущее читают как правду.
    #[test]
    fn unknown_level_and_gap_are_absent_not_false() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        let d = &trade_of(&evs).data;
        assert!(d.get("level").is_none(), "уровня не было — поля нет");
        assert!(
            d.get("acrossGap").is_none(),
            "про разрыв неизвестно — поля нет, а не false"
        );
    }

    /// Оба поля ОПЦИОНАЛЬНЫ, и остальная часть события без них полноценна:
    /// строгое поле обнулило бы разбор событий, уже лежащих в очередях.
    #[test]
    fn trade_without_optional_fields_is_still_complete() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        let d = &trade_of(&evs).data;
        for key in [
            "tenant",
            "userId",
            "tradeId",
            "symbol",
            "side",
            "volume",
            "price",
            "executedAt",
        ] {
            assert!(d.get(key).is_some(), "обязательное поле {key} пропало");
        }
    }

    // ---- Адресация и конверт ----------------------------------------------

    #[test]
    fn envelope_addresses_the_site_and_its_user() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        let e = trade_of(&evs);
        assert_eq!(e.event, EV_TRADE_EXECUTED);
        assert_eq!(e.version, EVENT_VERSION);
        assert_eq!(e.source, SOURCE);
        assert_eq!(e.occurred_at, "2026-01-01T00:00:00.123Z");
        assert_eq!(e.data["tenant"], TENANT);
        assert_eq!(
            e.data["userId"], SUB,
            "адресат — пользователь САЙТА, не наш номер счёта"
        );
        assert_eq!(
            e.data["symbol"], "BTCUSD",
            "написание платформы, не ETH-USDT и не номер"
        );
        assert_eq!(
            e.data["side"], "buy",
            "сторона СДЕЛКИ, а не закрывающей ноги"
        );
    }

    /// Идентификатор выведен из факта: та же сделка — тот же `event_id`.
    ///
    /// Со случайным `uuid4` повторная публикация того же факта уехала бы вторым
    /// событием, и дедуп потребителя (по `event_id`) не спас бы: он дедуплицирует
    /// одинаковые, а не похожие.
    #[test]
    fn event_id_is_derived_from_the_fact() {
        let once = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        let twice = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        assert_eq!(trade_of(&once).event_id, trade_of(&twice).event_id);
        assert_ne!(
            event_id(EV_TRADE_EXECUTED, "7:0"),
            event_id(EV_TRADE_EXECUTED, "7:1"),
            "разные сделки — разные идентификаторы"
        );
        assert_ne!(
            event_id(EV_TRADE_EXECUTED, "7:0"),
            event_id(EV_BALANCE_CHANGED, "7:0"),
            "разные события одного счёта не сливаются"
        );
        // Значение закреплено константой, и константа посчитана НЕ нашим кодом:
        // `uuid.uuid5(uuid.NAMESPACE_URL, "urn:terminal:terminal.trade.executed:7:0")`
        // в стандартной библиотеке Python. Иначе проверка сравнивала бы реализацию
        // с самой собой. Смена пространства имён или формы ключа сменила бы
        // идентификаторы всем событиям разом, и дедуп у потребителя пропустил бы
        // старые факты как новые — молча.
        assert_eq!(
            event_id(EV_TRADE_EXECUTED, "7:0"),
            "f34e50af-4fa9-5d60-9167-a84bfd8c5a60"
        );
    }

    // ---- Счёт заведён, остаток изменился ----------------------------------

    #[test]
    fn account_opened_comes_from_the_journal_not_from_guessing() {
        let evs = envelopes_for(
            &who(1),
            &before_empty(),
            &empty_snap(),
            &[EventKind::AccountOpened {
                balance: 1_000_000,
                profile: Default::default(),
            }],
            &known,
        );
        let e = evs
            .iter()
            .find(|e| e.event == EV_ACCOUNT_OPENED)
            .expect("заведение счёта обязано уехать");
        assert_eq!(e.data["balanceCents"], serde_json::json!(1_000_000));
        assert_eq!(e.data["currency"], "USD");
    }

    #[test]
    fn balance_change_carries_the_positions_snapshot() {
        let mut after = after_one_deal(None, Some(false));
        after.positions = vec![Position {
            id: 2,
            instrument: 1,
            side: PosSide::Short,
            qty: 50_000,
            entry: 6_900_000,
            pd: 2,
            qd: 5,
            margin: 100_000,
            sl: None,
            tp: None,
            leverage: 1,
        }];
        let evs = envelopes_for(&who(9), &before_empty(), &after, &[], &known);
        let e = evs
            .iter()
            .find(|e| e.event == EV_BALANCE_CHANGED)
            .expect("остаток изменился — событие обязано быть");
        assert_eq!(e.data["balanceCents"], serde_json::json!(1_185_118));
        assert_eq!(e.data["reason"], "trade");
        let p = &e.data["positions"][0];
        assert_eq!(p["symbol"], "BTCUSD");
        assert_eq!(p["side"], "sell");
        assert_eq!(p["volume"], serde_json::json!(0.5));
        assert_eq!(p["entryPrice"], serde_json::json!(69_000.0));
    }

    /// Остаток не изменился — события о нём нет. Иначе проекция переписывалась бы
    /// тем же числом на каждое движение, и «баланс изменился» перестало бы что-то
    /// значить.
    #[test]
    fn unchanged_balance_produces_no_event() {
        let evs = envelopes_for(&who(3), &before_empty(), &empty_snap(), &[], &known);
        assert!(evs.iter().all(|e| e.event != EV_BALANCE_CHANGED));
    }

    /// Причина не называется наугад. Словарь у потребителя закрытый, поле
    /// опционально; `adjustment` вместо «не знаю» читалось бы как ручная проводка.
    #[test]
    fn reason_is_omitted_when_unknown() {
        let mut after = empty_snap();
        after.balance = 900_000;
        let evs = envelopes_for(&who(4), &before_empty(), &after, &[], &known);
        let e = evs
            .iter()
            .find(|e| e.event == EV_BALANCE_CHANGED)
            .expect("остаток изменился");
        assert!(e.data.get("reason").is_none(), "причина не выдумывается");
    }

    /// **Мы не отправляем причину, которую потребитель не умеет прочитать.**
    ///
    /// Проверяется не список, а **каждый исход** `balance_reason`: список можно
    /// сверить глазами, а вот что функция не вернёт чего-то сверх него — нет.
    #[test]
    fn every_reason_we_can_emit_is_one_the_consumer_knows() {
        let before = before_empty();
        let opened = [EventKind::AccountOpened {
            balance: 1,
            profile: Default::default(),
        }];
        let deposited = [EventKind::Deposited { amount: 1 }];
        let traded = after_one_deal(None, None);
        let mut moved = empty_snap();
        moved.balance = 1;

        let cases: [(&[EventKind], &AccountSnapshot); 4] = [
            (&[], &moved),
            (&opened, &moved),
            (&deposited, &moved),
            (&[], &traded),
        ];
        for (kinds, after) in cases {
            if let Some(r) = balance_reason(kinds, &before, after) {
                assert!(
                    CONSUMER_KNOWN_REASONS.contains(&r),
                    "причина «{r}» потребителю неизвестна — событие станет malformed, и остаток потеряется"
                );
            }
        }
    }
    /// И сам предохранитель — **вызванный с тем значением, ради которого он
    /// поставлен**.
    ///
    /// Через `envelopes_for` до него не дотянуться: `balance_reason` отдаёт
    /// только известные причины, и проверка через неё доказывала бы, что
    /// предохранитель не мешает, а не что он срабатывает. Первая редакция
    /// именно это и делала — мутация «снять предохранитель» оставляла её
    /// зелёной.
    #[test]
    fn an_unknown_reason_is_not_put_on_the_wire() {
        assert_eq!(
            readable_reason("trade"),
            Some("trade"),
            "известную пропускаем"
        );
        assert_eq!(
            readable_reason("compensation"),
            None,
            "а незнакомую — нет: она уронила бы разбор всего события, и остаток потерялся бы вместе с причиной"
        );
        assert_eq!(readable_reason(""), None);
        assert_eq!(
            readable_reason("TRADE"),
            None,
            "перечень точный, а не похожий"
        );
    }

    /// **Тройка едет рядом со старым полем и несёт то же число** (Р-293, шаг 1).
    ///
    /// Проверяется именно совпадение: пока валюта одна, оно обязано быть, и
    /// расхождение здесь означало бы, что одна из двух дорог врёт уже сейчас.
    #[test]
    fn money_travels_as_amount_currency_and_precision() {
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, Some(false)),
            &[],
            &known,
        );
        let b = evs
            .iter()
            .find(|e| e.event == EV_BALANCE_CHANGED)
            .expect("событие об остатке");
        assert_eq!(
            b.data["balanceMinor"], b.data["balanceCents"],
            "то же число"
        );
        assert_eq!(b.data["currency"], "USD");
        assert_eq!(b.data["currencyMinorUnits"], 2);

        let t = trade_of(&evs);
        assert_eq!(t.data["realizedPnlMinor"], t.data["realizedPnlCents"]);
        assert_eq!(t.data["currencyMinorUnits"], 2);
    }

    /// **Старое поле остаётся на месте.** Снять его раньше, чем потребитель
    /// перейдёт на тройку, значит уронить разбор — и потерять вместе с суммой
    /// всё событие.
    #[test]
    fn the_old_field_is_still_sent() {
        let evs = envelopes_for(
            &who(1),
            &before_empty(),
            &empty_snap(),
            &[EventKind::AccountOpened {
                balance: 1_000_000,
                profile: Default::default(),
            }],
            &known,
        );
        let e = evs
            .iter()
            .find(|e| e.event == EV_ACCOUNT_OPENED)
            .expect("заведение счёта");
        assert_eq!(e.data["balanceCents"], serde_json::json!(1_000_000));
        assert_eq!(e.data["balanceMinor"], serde_json::json!(1_000_000));
    }

    /// Сброс демо-счёта называет себя причиной `reset` — она уже есть в
    /// перечне потребителя, значит доедет и ничего не уронит.
    #[test]
    fn a_demo_reset_is_reported_as_reset() {
        let mut after = empty_snap();
        after.balance = 1_000_000;
        let evs = envelopes_for(
            &who(7),
            &Before {
                closed: 0,
                balance: 900_000,
            },
            &after,
            &[EventKind::DemoAccountReset {
                adjustment: 100_000,
            }],
            &known,
        );
        let e = evs
            .iter()
            .find(|e| e.event == EV_BALANCE_CHANGED)
            .expect("остаток изменился");
        assert_eq!(e.data["reason"], "reset");
        assert!(
            CONSUMER_KNOWN_REASONS.contains(&"reset"),
            "и эта причина потребителю известна"
        );
    }

    // ---- Отказ, похожий на данные ------------------------------------------

    /// Неизвестный инструмент — события НЕТ. Выдуманный символ в ленте кабинета
    /// хуже отсутствующей строки: его не отличить от настоящего.
    #[test]
    fn unknown_instrument_yields_no_trade_event() {
        let mut after = after_one_deal(None, None);
        after.closed[0].instrument = 999;
        let evs = envelopes_for(&who(5), &before_empty(), &after, &[], &known);
        assert!(
            evs.iter().all(|e| e.event != EV_TRADE_EXECUTED),
            "символ выдумывать нельзя"
        );
    }

    // ---- Порядок повторной доставки ----------------------------------------

    #[test]
    fn backoff_grows_and_stops_at_an_hour() {
        assert_eq!(next_delay_ms(1), 5_000);
        assert_eq!(next_delay_ms(2), 25_000);
        assert_eq!(next_delay_ms(3), 125_000);
        assert_eq!(next_delay_ms(4), 625_000);
        // Сетка выписана поимённо, а не формулой: формула в проверке повторила бы
        // реализацию и не поймала бы смену основания. Числа те же, что у релея
        // платформы (`BASE * 5^(attempts-1)`, потолок час) — одинаковый закон
        // повторов в одном контуре был выбран намеренно.
        assert_eq!(
            next_delay_ms(5),
            3_125_000,
            "пятая ещё не упёрлась в потолок"
        );
        assert_eq!(next_delay_ms(6), 3_600_000, "шестая — потолок, час");
        assert_eq!(
            next_delay_ms(50),
            3_600_000,
            "и дальше час, а не переполнение"
        );
        assert_eq!(
            next_delay_ms(0),
            5_000,
            "нулевая попытка не уводит отступ в ноль"
        );
    }

    #[test]
    fn iso_is_millisecond_utc() {
        assert_eq!(iso_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_millis(1_767_225_600_123), "2026-01-01T00:00:00.123Z");
    }

    #[test]
    fn platform_symbol_matches_the_allow_list_spelling() {
        assert_eq!(platform_symbol(&btc()), "BTCUSD");
    }

    /// `CloseCause` в событие не едет: у потребителя такого поля нет, и
    /// добавление его в одностороннем порядке было бы изобретением контракта.
    /// Тест существует, чтобы следующий не «дозакрыл» это по аналогии с журналом.
    #[test]
    fn close_cause_stays_inside() {
        let _ = CloseCause::StopLoss;
        let evs = envelopes_for(
            &who(5),
            &before_empty(),
            &after_one_deal(None, None),
            &[],
            &known,
        );
        assert!(trade_of(&evs).data.get("cause").is_none());
    }
}
