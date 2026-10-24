//! # broker
//!
//! Бумажный брокер (ADR-014): счета с виртуальным балансом, позиции Long/Short с SL/TP, отложенные
//! (лимитные) ордера с триггером по цене, история закрытых сделок, маржа и P&L на реальных ценах.
//! Деньги — в **центах** (`i128`). Реальных денег нет.

use std::collections::HashMap;

use domain::account::UserId;

pub mod journal;

use journal::{CloseCause, EventKind};

/// Деньги в центах (1/100 USD).
pub type Cents = i128;

/// Сторона позиции/ордера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosSide {
    Long,
    Short,
}

/// Стоимость `price · qty` в центах: `price_raw · qty_raw · 100 / 10^(pd+qd)`.
fn value_cents(price_raw: i64, qty_raw: i64, pd: u8, qd: u8) -> Cents {
    price_raw as i128 * qty_raw as i128 * 100 / 10i128.pow(pd as u32 + qd as u32)
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
pub fn realized_on_close(side: PosSide, entry: i64, exit: i64, qty: i64, pd: u8, qd: u8) -> Cents {
    let diff = match side {
        PosSide::Long => exit - entry,
        PosSide::Short => entry - exit,
    };
    value_cents(diff, qty, pd, qd)
}

/// Маржа под объём `qty` по цене `entry` при плече `lev` — та же формула, что при
/// открытии. Отдельной функцией, чтобы неттинг и открытие не разъехались.
pub fn margin_for(entry: i64, qty: i64, pd: u8, qd: u8, lev: i128) -> Cents {
    value_cents(entry, qty, pd, qd) / lev.max(1)
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
}

impl Position {
    /// Нереализованный P&L при марк-цене.
    pub fn unrealized(&self, mark_raw: i64) -> Cents {
        let diff = match self.side {
            PosSide::Long => mark_raw - self.entry,
            PosSide::Short => self.entry - mark_raw,
        };
        value_cents(diff, self.qty, self.pd, self.qd)
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerError {
    InsufficientMargin,
    UnknownPosition,
    UnknownPending,
    /// Вывод больше, чем свободно (за вычетом занятого под маржу).
    InsufficientFunds,
}

#[derive(Debug, Default, Clone)]
struct Account {
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
            .map(|p| p.unrealized(*marks.get(&p.instrument).unwrap_or(&p.entry)))
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

    #[allow(clippy::too_many_arguments)]
    fn try_open(
        &mut self,
        lev: i128,
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
        match self.model {
            PositionModel::Hedging => {
                self.open_new(lev, marks, instrument, side, qty, entry, pd, qd, sl, tp)
            }
            PositionModel::Netting => {
                self.open_netting(lev, marks, instrument, side, qty, entry, pd, qd, sl, tp)
            }
        }
    }

    /// Хеджирование: каждая сделка — новая независимая позиция.
    #[allow(clippy::too_many_arguments)]
    fn open_new(
        &mut self,
        lev: i128,
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
        let margin = margin_for(entry, qty, pd, qd, lev);
        // Свободная маржа считается ОДНИМ способом — через equity (TRM-41).
        if self.free_margin(marks) < margin {
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
                entry,
                pd,
                qd,
                margin,
                sl,
                tp,
            },
        );
        self.log(EventKind::PositionOpened {
            id,
            instrument,
            side,
            qty,
            entry,
            pd,
            qd,
            sl,
            tp,
        });
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
    #[allow(clippy::too_many_arguments)]
    fn open_netting(
        &mut self,
        lev: i128,
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
        let existing = self
            .positions
            .values()
            .find(|p| p.instrument == instrument)
            .map(|p| p.id);
        let Some(pos_id) = existing else {
            return self.open_new(lev, marks, instrument, side, qty, entry, pd, qd, sl, tp);
        };
        let cur = self
            .positions
            .get(&pos_id)
            .expect("позиция только что найдена")
            .clone();

        if cur.side == side {
            // Доливка: объёмы складываются, вход усредняется по объёму.
            let new_qty = cur.qty + qty;
            let new_entry = merged_entry(cur.qty, cur.entry, qty, entry);
            let new_margin = margin_for(new_entry, new_qty, pd, qd, lev);
            // Считаем ДОПОЛНИТЕЛЬНУЮ маржу: часть уже занята этой же позицией.
            let extra = new_margin - cur.margin;
            // Тем же единственным определением, что и обычное открытие (TRM-41):
            // доливка — это увеличение риска, и считаться она обязана так же.
            if extra > 0 && self.free_margin(marks) < extra {
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
                add_entry: entry,
                sl,
                tp,
            });
            return Ok(pos_id);
        }

        // Встречная сделка: сворачиваем меньшую из величин.
        let closing = qty.min(cur.qty);
        let pnl = realized_on_close(cur.side, cur.entry, entry, closing, cur.pd, cur.qd);
        self.balance += pnl;
        self.closed.push(ClosedDeal {
            instrument: cur.instrument,
            side: cur.side,
            qty: closing,
            entry: cur.entry,
            exit: entry,
            pnl,
            pd: cur.pd,
            qd: cur.qd,
            // Неттинг свернул позицию встречной сделкой: уровня тут не было, а
            // разрыв ряда ни при чём — закрыл не триггер, а другая сделка.
            level: None,
            across_gap: Some(false),
        });

        let left = cur.qty - closing;
        if left > 0 {
            // Позиция уменьшилась, направление прежнее. Вход не меняется: свёрнута
            // часть, а не перезаключена сделка.
            let p = self.positions.get_mut(&pos_id).expect("позиция на месте");
            p.qty = left;
            p.margin = margin_for(p.entry, left, p.pd, p.qd, lev);
            self.log(EventKind::PositionReduced {
                id: pos_id,
                qty: closing,
                exit: entry,
            });
            return Ok(pos_id);
        }

        self.positions.remove(&pos_id);
        self.log(EventKind::PositionClosed {
            id: pos_id,
            exit: entry,
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
                lev, marks, instrument, side, remainder, entry, pd, qd, sl, tp,
            );
        }
        Ok(pos_id)
    }

    /// `cause` — не косметика: по журналу надо отличать «человек закрыл» от «сработал
    /// стоп». Иначе при разборе претензии «меня закрыли не по моей команде» ответить
    /// будет нечем, а это ровно тот разговор, ради которого журнал и заводится.
    /// `exec` — цена ИСПОЛНЕНИЯ: по ней считаются деньги, и у триггеров это
    /// наблюдённая рыночная цена (Р-178, проскальзывание). `level` — уровень,
    /// который ставил человек; у закрытия по команде уровня нет, там `None`.
    ///
    /// Обе нужны вместе: по одной `exec` нельзя отличить «рынок дошёл ровно до
    /// стопа» от «рынок перепрыгнул стоп на разрыве», а это разные разговоры
    /// с клиентом (TRM-31).
    fn close_pos(
        &mut self,
        id: u64,
        exec: i64,
        level: Option<i64>,
        across_gap: bool,
        cause: CloseCause,
    ) -> Option<Cents> {
        let pos = self.positions.remove(&id)?;
        let pnl = pos.unrealized(exec);
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
    fn run_triggers(
        &mut self,
        lev: i128,
        marks: &HashMap<u32, i64>,
        across_gap: bool,
        stop_out_pct: Option<i128>,
        protect_negative: bool,
    ) -> bool {
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
            self.log(EventKind::PendingTriggered {
                id: p.id,
                price: p.price,
                exec: Some(exec),
                across_gap,
            });
            let _ = self.try_open(
                lev,
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
                let sl_hit = pos.sl.is_some_and(|sl| match pos.side {
                    PosSide::Long => m <= sl,
                    PosSide::Short => m >= sl,
                });
                let tp_hit = pos.tp.is_some_and(|tp| match pos.side {
                    PosSide::Long => m >= tp,
                    PosSide::Short => m <= tp,
                });
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
            self.close_pos(id, exec, Some(level), across_gap, cause);
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
        let stopped_out = self.run_stop_out(marks, across_gap, stop_out_pct);
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
    ) -> bool {
        let Some(threshold) = stop_out_pct else {
            return false;
        };
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
                        .map(|m| (p.id, *m, p.unrealized(*m)))
                })
                .min_by_key(|(_, _, pnl)| *pnl);
            let Some((id, mark, _)) = worst else {
                break; // цен нет ни по одной — закрывать нечем
            };
            // Уровня человек здесь не ставил: закрытие наше, и называть
            // что-либо «уровнем» было бы приписыванием ему чужого решения.
            self.close_pos(id, mark, None, across_gap, CloseCause::StopOut);
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
        self.accounts.entry(user).or_insert_with(|| {
            // Заведение счёта — тоже изменение состояния, и стартовый баланс обязан
            // иметь происхождение. Без этой записи первые деньги на счёте взялись бы
            // ниоткуда, и журнал начинался бы с необъяснимой суммы.
            let mut a = Account {
                balance: start,
                ..Default::default()
            };
            a.log(EventKind::AccountOpened { balance: start });
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
        let fresh = !self.accounts.contains_key(&user);
        let _ = self.acct(user);
        fresh
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

    pub fn balance(&self, user: UserId) -> Cents {
        self.accounts
            .get(&user)
            .map(|a| a.balance)
            .unwrap_or(self.start_balance)
    }
    pub fn used_margin(&self, user: UserId) -> Cents {
        self.accounts
            .get(&user)
            .map(|a| a.used_margin())
            .unwrap_or(0)
    }
    /// Свободная маржа — через equity, **одним определением на весь счёт**
    /// (TRM-41). Требует цены: нереализованный результат — часть средств.
    pub fn free_margin(&self, user: UserId, marks: &HashMap<u32, i64>) -> Cents {
        self.accounts
            .get(&user)
            .map(|a| a.free_margin(marks))
            .unwrap_or(self.start_balance)
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
                    .map(|p| p.unrealized(*marks.get(&p.instrument).unwrap_or(&p.entry)))
                    .sum()
            })
            .unwrap_or(0)
    }
    pub fn equity(&self, user: UserId, marks: &HashMap<u32, i64>) -> Cents {
        self.balance(user) + self.open_pnl(user, marks)
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
        let lev = self.leverage;
        self.acct(user)
            .try_open(lev, marks, instrument, side, qty, entry, pd, qd, sl, tp)
    }

    /// Закрыть позицию по марк-цене. Возвращает реализованный P&L.
    pub fn close(&mut self, user: UserId, id: u64, mark: i64) -> Result<Cents, BrokerError> {
        // Закрытие по команде исполняется по той же цене, которую видели:
        // расхождения нет, и `level` здесь молчит — не потому что неизвестно,
        // а потому что назвать нечего.
        self.acct(user)
            .close_pos(id, mark, None, false, CloseCause::Command)
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
        if amount > self.free_margin(user, marks) {
            return Err(BrokerError::InsufficientFunds);
        }
        let a = self.acct(user);
        a.balance -= amount;
        a.log(EventKind::Withdrawn { amount });
        Ok(a.balance)
    }

    /// Разместить отложенный (лимитный) ордер. Возвращает его id.
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
    ) -> u64 {
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
        id
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
        let lev = self.leverage;
        let stop_out = self.stop_out_pct;
        let protect = self.protect_negative_balance;
        let mut changed = Vec::new();
        for (user, a) in self.accounts.iter_mut() {
            if a.run_triggers(lev, marks, across_gap, stop_out, protect) {
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
                balance: a.balance,
                next_id: a.next_id,
                positions: a.positions.values().cloned().collect(),
                pendings: a.pendings.values().cloned().collect(),
                closed: a.closed.clone(),
                model: a.model,
            },
            None => AccountSnapshot {
                balance: self.start_balance,
                next_id: 0,
                positions: vec![],
                pendings: vec![],
                closed: vec![],
                model: PositionModel::default(),
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
            balance: snap.balance,
            next_id: snap.next_id,
            positions: snap.positions.into_iter().map(|p| (p.id, p)).collect(),
            pendings: snap.pendings.into_iter().map(|p| (p.id, p)).collect(),
            closed: snap.closed,
            model: snap.model,
            // Защёлка предупреждения состояние счёта не описывает и в слепке её
            // нет: после подъёма счёт, всё ещё лежащий ниже уровня, будет
            // предупреждён заново. Повтор верного предупреждения дешевле
            // столбца в денежной таблице ради состояния переписки (TRM-50).
            margin_called: false,
            outbox: Vec::new(),
        };
        self.accounts.insert(user, a);
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

    /// Плечо брокера — нужно тем, кто проверяет инварианты по слепку.
    pub fn leverage(&self) -> i128 {
        self.leverage
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
    pub balance: Cents,
    pub next_id: u64,
    pub positions: Vec<Position>,
    pub pendings: Vec<PendingOrder>,
    pub closed: Vec<ClosedDeal>,
    /// Модель ведения позиций счёта (ADR-025). Счета, заведённые до 2026-09-04,
    /// поднимаются как `Hedging` — другой модели в коде не существовало.
    pub model: PositionModel,
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

/// Проверить слепок счёта. Пустой вектор — всё сходится.
///
/// Чего здесь сознательно НЕТ: проверки `balance < 0`. Она была единственной и
/// фатальной, а с плечом > 1 отрицательный баланс законен — то есть проверка,
/// написанная под допущение `leverage = 1`, не давала бы сервису стартовать на
/// корректном состоянии в день включения плеча (TRM-06). Неверная проверка,
/// которая сегодня проходит, опаснее отсутствующей: она сработает позже и на
/// правильных данных.
pub fn check_snapshot(snap: &AccountSnapshot, leverage: i128) -> Vec<Violation> {
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
    let lev = leverage.max(1);
    for p in &snap.positions {
        let expected = value_cents(p.entry, p.qty, p.pd, p.qd) / lev;
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
        }
    }

    /// Маржа в фикстуре — не произвольная: она обязана сходиться с формулой
    /// `value_cents(entry, qty, pd, qd) / плечо`, иначе слепок «здоровым» не
    /// является. Первая версия этой фикстуры брала маржу с потолка, и новый
    /// инвариант поймал её сразу же — что и требовалось.
    fn healthy() -> AccountSnapshot {
        AccountSnapshot {
            balance: 1_000_000,
            next_id: 2,
            positions: vec![
                pos(1, 100, 5000, value_cents(5000, 100, 2, 3)),
                pos(2, 200, 4000, value_cents(4000, 200, 2, 3)),
            ],
            pendings: vec![],
            closed: vec![],
            model: PositionModel::Hedging,
        }
    }

    #[test]
    fn healthy_snapshot_has_no_violations() {
        assert_eq!(check_snapshot(&healthy(), 1), vec![]);
    }

    #[test]
    fn empty_account_is_valid() {
        assert_eq!(check_snapshot(&AccountSnapshot::default(), 1), vec![]);
    }

    /// Отрицательный баланс НЕ нарушение: с плечом он законен. Прежняя проверка
    /// считала его фатальным и не дала бы сервису стартовать (TRM-06).
    #[test]
    fn negative_balance_is_not_a_violation() {
        let mut s = healthy();
        s.balance = -50_000;
        assert_eq!(
            check_snapshot(&s, 1),
            vec![],
            "отрицательный баланс законен с плечом"
        );
    }

    #[test]
    fn detects_zero_and_negative_qty() {
        let mut s = healthy();
        s.positions[0].qty = 0;
        assert!(
            check_snapshot(&s, 1).iter().any(|v| v.0.contains("объём")),
            "нулевой объём должен ловиться"
        );
        s.positions[0].qty = -5;
        assert!(check_snapshot(&s, 1).iter().any(|v| v.0.contains("объём")));
    }

    #[test]
    fn detects_zero_entry_price() {
        let mut s = healthy();
        s.positions[1].entry = 0;
        assert!(check_snapshot(&s, 1)
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
            check_snapshot(&s, 1).iter().any(|v| v.0.contains("маржа")),
            "дрейф маржи должен ловиться"
        );
    }

    /// Та же маржа при другом плече — нарушение: слепок посчитан по чужим правилам.
    #[test]
    fn detects_margin_computed_with_other_leverage() {
        let s = healthy();
        assert!(
            !check_snapshot(&s, 10).is_empty(),
            "маржа под плечо 1 не годится для плеча 10"
        );
    }

    #[test]
    fn detects_negative_margin() {
        let mut s = healthy();
        s.positions[0].margin = -1;
        assert!(check_snapshot(&s, 1)
            .iter()
            .any(|v| v.0.contains("отрицательная маржа")));
    }

    #[test]
    fn detects_duplicate_ids() {
        let mut s = healthy();
        s.positions[1].id = s.positions[0].id;
        assert!(check_snapshot(&s, 1)
            .iter()
            .any(|v| v.0.contains("не уникальны")));
    }

    /// Счётчик id обязан покрывать выданные: иначе следующая позиция затрёт
    /// существующую, и одна из них исчезнет вместе со своей маржой.
    #[test]
    fn detects_id_beyond_counter() {
        let mut s = healthy();
        s.next_id = 1; // а позиция с id=2 уже есть
        assert!(check_snapshot(&s, 1)
            .iter()
            .any(|v| v.0.contains("next_id")));
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
        assert!(check_snapshot(&s, 1).iter().any(|v| v.0.contains("ордер")));
        s.pendings[0].qty = 10;
        s.pendings[0].price = 0;
        assert!(check_snapshot(&s, 1).iter().any(|v| v.0.contains("цена")));
    }

    /// Инварианты обязаны считаться и без хранилища: они чистые функции.
    /// Этот тест — растяжка: если проверку снова унесут в БД, он покраснеет.
    #[test]
    fn invariants_do_not_need_a_store() {
        let b = Broker::new(1_000_000, 1);
        let snap = b.snapshot(UserId(42)); // счёта нет — слепок по умолчанию
        assert_eq!(check_snapshot(&snap, 1), vec![]);
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
        assert_eq!(check_snapshot(&b.snapshot(u), 1), vec![], "после открытия");
        b.place_pending(u, 2, PosSide::Short, 500, 40_000, 2, 3, None, None);
        assert_eq!(
            check_snapshot(&b.snapshot(u), 1),
            vec![],
            "после лимитного ордера"
        );
        b.close(u, id, 51_000).expect("закрытие");
        assert_eq!(check_snapshot(&b.snapshot(u), 1), vec![], "после закрытия");
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
        assert_eq!(check_snapshot(&snap, 1), vec![]);
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
            realized_on_close(PosSide::Long, 5300, 5600, 150, PD, QD),
            45
        );
        // Шорт зеркально: прибыль при падении.
        assert_eq!(
            realized_on_close(PosSide::Short, 5600, 5300, 150, PD, QD),
            45
        );
        assert_eq!(
            realized_on_close(PosSide::Short, 5300, 5600, 150, PD, QD),
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
        let before = b.balance(u);

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
            b.balance(u) - before,
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
        let before = b.balance(u);

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
            b.balance(u) - before,
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
        assert_eq!(check_snapshot(&b.snapshot(u), 1), vec![], "после открытия");
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
            check_snapshot(&b.snapshot(u), 1),
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
        assert_eq!(
            check_snapshot(&b.snapshot(u), 1),
            vec![],
            "после переворота"
        );
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
        b.place_pending(u, 1, PosSide::Long, 100, 4000, PD, QD, None, None);
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
