use broker::journal::{AccountEvent, EventKind, Journal, Operator, Origin};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    EmptyJournal,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub through_seq: u64,
    pub sealed: Vec<AccountEvent>,
}

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

    const START: i128 = 1_000_000;

    fn operator() -> Operator {
        Operator::new("ops@platform", "решение владельца 24.10", "TRM-48").unwrap()
    }

    fn journal_with_history() -> Journal {
        let mut j = Journal::new(START);
        let kinds = [
            EventKind::AccountOpened {
                balance: START,
                profile: Default::default(),
            },
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
                lev: 1,
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

    #[test]
    fn the_boundary_cannot_be_dictated_from_outside() {
        let j = journal_with_history();
        let d = declare(&j, 1, operator()).unwrap();
        assert!(
            d.through_seq < d.sealed[0].seq(),
            "заявление не покрывает само себя"
        );
    }

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
        let j = Journal::new(START);
        assert_eq!(declare(&j, 1, operator()), Err(Refusal::EmptyJournal));
    }

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
