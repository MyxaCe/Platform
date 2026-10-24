//! # exclusion — история счёта исключается из отчётности (TRM-48)
//!
//! Решение владельца 24.10: три сделки, сделанные на общем демо-счёте, в реальные
//! балансы и отчётность не входят. Приписывать их человеку нельзя — из данных не
//! восстанавливается, кому.
//!
//! ## Форма продиктована нашими же замками
//!
//! Это первый раз, когда мы решаем судьбу **уже записанного** в неизменяемый
//! журнал. Удалить или переписать прошлое нельзя не «по правилу», а физически:
//! `account_events` закрыта триггером БД от `UPDATE`/`DELETE`, а записи сцеплены
//! отпечатками — правка разорвала бы цепочку, то есть испортила бы счёт сильнее
//! того, что мы исправляем.
//!
//! Поэтому судьба старых записей решается **новой записью об их природе**.
//!
//! ## Почему здесь, а не в бинаре
//!
//! Применяет объявление сегодня отдельный бинарь (`exclude-history`), а не ручка:
//! ручки оператора (ADM-03) не существует, сетевую поверхность ради одной
//! разовой операции открывать нечем, и бинарь **заставляет назвать аргументы
//! явно** — кто, на каком основании, по какому случаю. Это ровно тот след,
//! которого мы хотим от оператора; кнопка его не оставляет.
//!
//! Когда ADR-03 появится, ручка позовёт **этот** код, а не напишет свой. Поэтому
//! решение живёт в библиотеке, а бинарь — тонкая оболочка над ним.

use broker::journal::{AccountEvent, EventKind, Journal, Operator, Origin};

/// Почему объявление не может быть записано.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Журнал пуст: объявлять нечего, границы не существует.
    EmptyJournal,
    /// Объявление уже есть. Повтор запрещён: второе заявление о том же
    /// поверх первого означало бы, что граница подвижна, — а она и есть
    /// всё содержание записи.
    AlreadyDeclared { through_seq: u64 },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::EmptyJournal => {
                f.write_str("журнал счёта пуст — объявлять песочницей нечего")
            }
            Refusal::AlreadyDeclared { through_seq } => write!(
                f,
                "история уже объявлена песочницей по запись №{through_seq} — повторное заявление сдвинуло бы границу"
            ),
        }
    }
}

/// Что будет записано. Отдаётся наружу до записи, чтобы оператор увидел решение
/// глазами: разовая правка денежного журнала не делается вслепую.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    /// Граница заявления — номер последней существующей записи.
    pub through_seq: u64,
    /// Запечатанная запись, готовая к сохранению.
    pub sealed: Vec<AccountEvent>,
}

/// Подготовить объявление для журнала счёта.
///
/// Ничего не записывает: решает и запечатывает. Запись — отдельным шагом у
/// вызывающего, и между ними стоит показ человеку.
///
/// `through_seq` берётся из ленты, а не из аргументов: граница — это факт о
/// журнале, и давать её вводить руками значило бы разрешить объявить песочницей
/// то, чего ещё нет.
pub fn declare(journal: &Journal, at_ms: i64, operator: Operator) -> Result<Declaration, Refusal> {
    if let Some(through_seq) = journal.state().excluded_from_reporting_through {
        return Err(Refusal::AlreadyDeclared { through_seq });
    }
    let through_seq = journal
        .events()
        .last()
        .map(|e| e.seq())
        .ok_or(Refusal::EmptyJournal)?;

    let sealed = journal.seal(
        at_ms,
        &Origin::Operator(operator),
        &[EventKind::HistoryExcludedFromReporting { through_seq }],
    );
    Ok(Declaration {
        through_seq,
        sealed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use broker::PosSide;

    const LEV: i128 = 1;
    const START: i128 = 1_000_000;

    fn operator() -> Operator {
        Operator::new("ops@platform", "решение владельца 24.10", "TRM-48").unwrap()
    }

    /// Журнал с тремя записями — как у счёта на контуре.
    fn journal_with_history() -> Journal {
        let mut j = Journal::new(LEV, START);
        let kinds = [
            EventKind::AccountOpened { balance: START },
            EventKind::PositionOpened {
                id: 1,
                instrument: 2,
                side: PosSide::Long,
                qty: 1000,
                entry: 239_883,
                pd: 2,
                qd: 4,
                sl: None,
                tp: None,
            },
        ];
        for e in j.seal(1_700_000_000_000, &Origin::Client, &kinds) {
            j.commit(e).unwrap();
        }
        j
    }

    #[test]
    fn the_boundary_is_the_last_existing_entry() {
        let j = journal_with_history();
        let d = declare(&j, 1_700_000_100_000, operator()).expect("объявление");
        assert_eq!(d.through_seq, 2, "граница — последняя существующая запись");
        assert_eq!(d.sealed.len(), 1, "объявление — одна запись");
        assert_eq!(
            d.sealed[0].seq(),
            3,
            "и она продолжает ленту, а не встаёт в неё"
        );
        assert_eq!(
            d.sealed[0].kind(),
            &EventKind::HistoryExcludedFromReporting { through_seq: 2 }
        );
    }

    /// Граница берётся из ленты, а не из аргументов: иначе песочницей можно было
    /// бы объявить то, чего ещё нет.
    #[test]
    fn the_boundary_cannot_be_dictated_from_outside() {
        let j = journal_with_history();
        let d = declare(&j, 1, operator()).unwrap();
        assert!(
            d.through_seq < d.sealed[0].seq(),
            "заявление не покрывает само себя"
        );
    }

    /// Основание едет вместе с записью: ручная проводка без причины неотличима
    /// от злоупотребления.
    #[test]
    fn the_operator_travels_with_the_record() {
        let j = journal_with_history();
        let d = declare(&j, 1, operator()).unwrap();
        let op = d.sealed[0]
            .origin()
            .operator()
            .expect("источник обязан быть оператором");
        assert_eq!(op.who(), "ops@platform");
        assert_eq!(op.incident(), "TRM-48");
        assert!(!op.reason().is_empty());
    }

    #[test]
    fn an_empty_journal_has_nothing_to_declare() {
        let j = Journal::new(LEV, START);
        assert_eq!(declare(&j, 1, operator()), Err(Refusal::EmptyJournal));
    }

    /// Повтор запрещён: подвижная граница — это отсутствие границы.
    #[test]
    fn a_second_declaration_is_refused() {
        let mut j = journal_with_history();
        let d = declare(&j, 1, operator()).unwrap();
        for e in d.sealed {
            j.commit(e).unwrap();
        }
        assert_eq!(
            declare(&j, 2, operator()),
            Err(Refusal::AlreadyDeclared { through_seq: 2 })
        );
    }

    /// **Деньги объявление не трогает.** Свёртка его не применяет, и остаток
    /// после записи обязан остаться прежним — иначе пометка переписала бы счёт.
    #[test]
    fn declaring_changes_no_money() {
        let mut j = journal_with_history();
        let before = j.state().clone();
        let d = declare(&j, 1, operator()).unwrap();
        for e in d.sealed {
            j.commit(e).unwrap();
        }
        let after = j.state();

        assert_eq!(after.balance, before.balance, "остаток");
        assert_eq!(after.positions, before.positions, "позиции");
        assert_eq!(after.pendings, before.pendings, "ордера");
        assert_eq!(after.closed, before.closed, "история сделок");
        assert_eq!(after.next_id, before.next_id, "счётчик id");
        assert_eq!(after.model, before.model, "модель");
        assert_eq!(after.opened, before.opened, "происхождение");
        assert_eq!(after.orphans, before.orphans, "сироты");
        assert_eq!(
            after.excluded_from_reporting_through,
            Some(2),
            "изменилось РОВНО одно — заявление"
        );
    }
}
