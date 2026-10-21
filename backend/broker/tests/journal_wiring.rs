//! Связка «операция → запись в журнале → восстановленное состояние» (TRM-13).
//!
//! Юнит-тесты журнала проверяют журнал, юнит-тесты брокера — брокер. Ни те, ни другие
//! не ловят самый вероятный дефект: **операция есть, а записи о ней нет**. Код,
//! который написан и не вызывается, модульными тестами не ловится в принципе — это мы
//! уже видели на мутации пустого инварианта (TRM-06).
//!
//! Поэтому здесь каждая операция прогоняется через настоящий брокер, её события
//! складываются в настоящий журнал, журнал сворачивается обратно — и результат
//! сверяется со слепком счёта. Любая несписанная операция немедленно расходится.

use broker::journal::{audit, CloseCause, EventKind, Journal, JournalStatus, Origin};
use broker::{Broker, PosSide, PositionModel};
use domain::account::UserId;

use std::collections::HashMap;

const PD: u8 = 2;
const QD: u8 = 3;
const START: i128 = 10_000_000;
const U: UserId = UserId(1);

struct Rig {
    b: Broker,
    j: Journal,
    at: i64,
}

impl Rig {
    fn new() -> Self {
        Self {
            b: Broker::new(START, 1),
            j: Journal::new(1, START),
            at: 1_700_000_000_000,
        }
    }

    /// Снять события последней операции и записать их в журнал — ровно то, что делает
    /// шлюз. Источник приходит снаружи: ядро о нём не знает.
    fn record(&mut self, origin: Origin) {
        self.at += 1;
        let kinds = self.b.take_events(U);
        for e in self.j.seal(self.at, &origin, &kinds) {
            self.j.commit(e).expect("запись обязана продолжать ленту");
        }
    }

    /// Журнал обязан сходиться со слепком после каждого шага. Проверяем **пересчётом
    /// с нуля**, а не поддерживаемым состоянием: иначе сравнивали бы свёртку с собой.
    fn must_reconcile(&self, step: &str) {
        let snap = self.b.snapshot(U);
        let status = audit(&self.j, &snap);
        assert_eq!(status, JournalStatus::Reconciled, "{step}: {status}");
        assert_eq!(
            self.j.replay().balance,
            snap.balance,
            "{step}: остаток обязан выводиться из журнала"
        );
    }

    fn kinds(&self) -> Vec<&EventKind> {
        self.j.events().iter().map(|e| e.kind()).collect()
    }
}

// ---- Хеджирование: открытие и закрытие -------------------------------------

#[test]
fn opening_and_closing_are_recorded_and_replay_to_the_same_account() {
    let mut r = Rig::new();
    let id =
        r.b.open(U, 1, PosSide::Long, 1000, 50_000, PD, QD, None, None)
            .unwrap();
    r.record(Origin::Client);
    r.must_reconcile("после открытия");

    r.b.close(U, id, 51_000).unwrap();
    r.record(Origin::Client);
    r.must_reconcile("после закрытия");

    // Первая запись — заведение счёта: стартовый баланс обязан иметь происхождение.
    assert!(matches!(r.kinds()[0], EventKind::AccountOpened { balance } if *balance == START));
    assert!(matches!(r.kinds()[1], EventKind::PositionOpened { .. }));
    assert!(
        matches!(
            r.kinds()[2],
            EventKind::PositionClosed {
                cause: CloseCause::Command,
                ..
            }
        ),
        "закрытие по команде обязано отличаться от срабатывания стопа"
    );
}

// ---- Неттинг: доливка, частичное сворачивание, переворот --------------------

#[test]
fn netting_add_partial_and_flip_are_each_recorded() {
    let mut r = Rig::new();
    assert!(r.b.adopt_model(U, PositionModel::Netting));
    r.record(Origin::Client);

    r.b.open(U, 1, PosSide::Long, 100, 5000, PD, QD, None, None)
        .unwrap();
    r.record(Origin::Client);
    r.must_reconcile("после открытия");

    r.b.open(U, 1, PosSide::Long, 300, 5400, PD, QD, None, None)
        .unwrap();
    r.record(Origin::Client);
    r.must_reconcile("после доливки");
    assert!(matches!(
        r.kinds().last().unwrap(),
        EventKind::PositionIncreased { add_qty: 300, .. }
    ));

    r.b.open(U, 1, PosSide::Short, 150, 5600, PD, QD, None, None)
        .unwrap();
    r.record(Origin::Client);
    r.must_reconcile("после частичного сворачивания");
    assert!(matches!(
        r.kinds().last().unwrap(),
        EventKind::PositionReduced {
            qty: 150,
            exit: 5600,
            ..
        }
    ));

    r.b.open(U, 1, PosSide::Short, 500, 5100, PD, QD, None, None)
        .unwrap();
    r.record(Origin::Client);
    r.must_reconcile("после переворота");
    // Переворот — это две записи: свёрнутая позиция и открытая в новую сторону.
    let tail = r.kinds();
    let n = tail.len();
    assert!(matches!(
        tail[n - 2],
        EventKind::PositionClosed {
            cause: CloseCause::Netting,
            ..
        }
    ));
    // Остаток = встречный объём минус свёрнутый: 500 − 250.
    assert!(matches!(
        tail[n - 1],
        EventKind::PositionOpened {
            side: PosSide::Short,
            qty: 250,
            ..
        }
    ));
}

// ---- Срабатывание стопа и тейка --------------------------------------------

#[test]
fn stop_loss_is_recorded_with_its_cause_and_the_balance_replays() {
    let mut r = Rig::new();
    r.b.open(
        U,
        1,
        PosSide::Long,
        1000,
        50_000,
        PD,
        QD,
        Some(49_000),
        None,
    )
    .unwrap();
    r.record(Origin::Client);

    let marks: HashMap<u32, i64> = HashMap::from([(1, 48_500)]);
    assert_eq!(r.b.check(&marks, false), vec![U], "стоп обязан сработать");
    // Источник — движок: человека в этот момент нет, и в записи этого не должно быть.
    r.record(Origin::System);
    r.must_reconcile("после срабатывания стопа");

    let last = r.j.events().last().unwrap();
    // Рынок на 48 500 при стопе 49 000 — это проскальзывание, и исполняется
    // оно по рынку (Р-178). Раньше здесь стояло `exit: 49_000`: тест
    // утверждал, что мы закрыли по уровню, то есть что разницу доплатил
    // брокер. Это и была та неограниченная гарантия, о которой никто не знал.
    assert!(
        matches!(
            last.kind(),
            EventKind::PositionClosed {
                cause: CloseCause::StopLoss,
                exit: 48_500,
                level: Some(49_000),
                ..
            }
        ),
        "ожидалось закрытие по рынку с записанным уровнем, а не {:?}",
        last.kind()
    );
    assert_eq!(
        last.origin(),
        &Origin::System,
        "сработавший стоп не бывает действием человека"
    );
}

#[test]
fn take_profit_is_recorded_separately_from_stop_loss() {
    let mut r = Rig::new();
    r.b.open(
        U,
        1,
        PosSide::Long,
        1000,
        50_000,
        PD,
        QD,
        None,
        Some(52_000),
    )
    .unwrap();
    r.record(Origin::Client);
    r.b.check(&HashMap::from([(1, 52_500)]), false);
    r.record(Origin::System);
    r.must_reconcile("после тейка");
    assert!(matches!(
        r.j.events().last().unwrap().kind(),
        EventKind::PositionClosed {
            cause: CloseCause::TakeProfit,
            ..
        }
    ));
}

// ---- Отложенные ордера ------------------------------------------------------

#[test]
fn pending_lifecycle_is_recorded_place_cancel_and_fill() {
    let mut r = Rig::new();
    let id =
        r.b.place_pending(U, 1, PosSide::Long, 100, 4000, PD, QD, None, None);
    r.record(Origin::Client);
    r.must_reconcile("после размещения ордера");

    r.b.cancel_pending(U, id).unwrap();
    r.record(Origin::Client);
    r.must_reconcile("после отмены ордера");

    r.b.place_pending(U, 1, PosSide::Long, 100, 4000, PD, QD, None, None);
    r.record(Origin::Client);
    r.b.check(&HashMap::from([(1, 3_900)]), false);
    r.record(Origin::System);
    r.must_reconcile("после исполнения ордера");

    let tail = r.kinds();
    let n = tail.len();
    assert!(
        matches!(tail[n - 2], EventKind::PendingTriggered { .. }),
        "сработавший ордер обязан быть виден"
    );
    assert!(
        matches!(tail[n - 1], EventKind::PositionOpened { .. }),
        "у открывшейся позиции свой id и своя запись"
    );
}

// ---- Деньги -----------------------------------------------------------------

#[test]
fn deposit_and_withdrawal_replay_into_the_same_balance() {
    let mut r = Rig::new();
    r.b.deposit(U, 500_000);
    r.record(Origin::Client);
    r.must_reconcile("после пополнения");
    r.b.withdraw(U, 200_000).unwrap();
    r.record(Origin::Client);
    r.must_reconcile("после вывода");
    assert_eq!(r.j.replay().balance, START + 300_000);
}

/// Ручная проводка центра управления (Р-049) — та же операция с другим источником.
/// Остаток **выводится** из неё, а не правится: это и есть форма, под которую
/// делается ADM-03.
#[test]
fn manual_posting_goes_through_the_same_path_and_is_distinguishable() {
    let mut r = Rig::new();
    let op = Origin::Operator(
        broker::journal::Operator::new(
            "ops@platform",
            "платёж прошёл на стороне провайдера",
            "INC-42",
        )
        .unwrap(),
    );
    r.b.deposit(U, 1_234_500);
    r.record(op);
    r.must_reconcile("после ручной проводки");

    let last = r.j.events().last().unwrap();
    let who = last
        .origin()
        .operator()
        .expect("проводка обязана остаться ручной");
    assert_eq!((who.who(), who.incident()), ("ops@platform", "INC-42"));
    assert_eq!(r.b.balance(U), START + 1_234_500);
    assert_eq!(
        r.j.replay().balance,
        r.b.balance(U),
        "остаток обязан сходиться с проводками"
    );
}

// ---- Отрицательный контроль -------------------------------------------------

/// Несписанная операция обязана обнаруживаться. Это и есть смысл сверки: без неё
/// «забыли записать» выглядит как нормальная работа.
#[test]
fn an_operation_that_was_not_recorded_is_detected() {
    let mut r = Rig::new();
    r.b.open(U, 1, PosSide::Long, 1000, 50_000, PD, QD, None, None)
        .unwrap();
    r.record(Origin::Client);
    r.must_reconcile("контроль: пока всё сходится");

    // Операция происходит, а записать её «забыли»: события выброшены.
    r.b.deposit(U, 999_999);
    let _dropped = r.b.take_events(U);

    let status = audit(&r.j, &r.b.snapshot(U));
    assert!(
        matches!(status, JournalStatus::Diverged(_)),
        "незаписанная операция обязана находиться: {status}"
    );
    assert!(
        status.to_string().contains("баланс"),
        "и причина обязана называть, что именно разошлось: {status}"
    );
}

/// Очередь событий сливается и при отказе: несостоявшаяся операция не должна уехать
/// в журнал прицепом к следующей.
#[test]
fn events_of_a_failed_operation_do_not_leak_into_the_next_one() {
    let mut r = Rig::new();
    r.record(Origin::Client); // счёта ещё нет — записи тоже
    assert!(
        r.b.open(
            U,
            1,
            PosSide::Long,
            i64::MAX / 2,
            50_000,
            PD,
            QD,
            None,
            None
        )
        .is_err(),
        "маржи не хватит"
    );
    let after_failure = r.b.take_events(U);
    assert!(
        after_failure
            .iter()
            .all(|k| !matches!(k, EventKind::PositionOpened { .. })),
        "отказавшее открытие не должно порождать запись об открытии"
    );

    r.b.open(U, 1, PosSide::Long, 100, 50_000, PD, QD, None, None)
        .unwrap();
    r.record(Origin::Client);
    r.must_reconcile("после удавшейся операции");
    assert_eq!(r.b.pending_events(U), 0, "очередь обязана быть пустой");
}

/// Длинная смешанная последовательность: каждый шаг сверяется, и в конце состояние
/// восстанавливается из ленты целиком.
#[test]
fn a_long_mixed_sequence_replays_to_the_same_account() {
    let mut r = Rig::new();
    r.b.deposit(U, 1_000_000);
    r.record(Origin::Client);
    let a =
        r.b.open(
            U,
            1,
            PosSide::Long,
            1000,
            50_000,
            PD,
            QD,
            Some(49_000),
            None,
        )
        .unwrap();
    r.record(Origin::Client);
    r.b.open(U, 2, PosSide::Short, 300, 20_000, PD, QD, None, None)
        .unwrap();
    r.record(Origin::Client);
    r.b.place_pending(U, 3, PosSide::Long, 50, 1_000, PD, QD, None, None);
    r.record(Origin::Client);
    r.b.check(&HashMap::from([(1, 48_000), (3, 900)]), false);
    r.record(Origin::System);
    r.b.withdraw(U, 10_000).unwrap();
    r.record(Origin::Client);
    let _ = a;
    r.must_reconcile("после смешанной последовательности");

    let snap = r.b.snapshot(U);
    let replayed = r.j.replay();
    assert_eq!(replayed.balance, snap.balance);
    assert_eq!(replayed.positions.len(), snap.positions.len());
    assert_eq!(replayed.closed.len(), snap.closed.len());
    assert!(
        r.j.len() > 6,
        "лента обязана содержать все шаги, а не последний"
    );
}
