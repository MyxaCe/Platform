//! Повторная доставка на шину (TRM-40): проверяется на **отказывающем
//! отправителе**, а не на отключённой шине.
//!
//! Разница существенная. Отключённая шина проверяет, что мы ничего не делаем;
//! отказывающий отправитель проверяет, что мы делаем правильное: оставляем строку
//! в очереди, записываем попытку и причину и приходим снова. Это два разных
//! свойства, и первое легко принять за второе.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use broker::journal::{EventKind, Journal, Origin};
use broker::AccountSnapshot;
use domain::account::UserId;
use gateway::bus::{relay_once, BusError, BusPublisher};
use gateway::persistence::{MemoryStore, OutboxRecord, Persistence};

const U: UserId = UserId(5);

/// Отправитель, который отказывает первые `fail_times` раз, а дальше принимает.
/// Так проверяется не «отказ» и не «успех», а переход от одного к другому —
/// то есть собственно восстановление.
struct Flaky {
    fail_times: usize,
    calls: AtomicUsize,
    accepted: Mutex<Vec<(String, String)>>,
}

impl Flaky {
    fn new(fail_times: usize) -> Self {
        Self {
            fail_times,
            calls: AtomicUsize::new(0),
            accepted: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl BusPublisher for Flaky {
    async fn publish(&self, rk: &str, event_id: &str, _payload: &str) -> Result<(), BusError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_times {
            return Err(BusError("шина недоступна".into()));
        }
        self.accepted
            .lock()
            .unwrap()
            .push((rk.to_string(), event_id.to_string()));
        Ok(())
    }
}

fn snap() -> AccountSnapshot {
    AccountSnapshot {
        balance: 10_000_000,
        next_id: 1,
        ..Default::default()
    }
}

fn opening(j: &Journal) -> Vec<broker::journal::AccountEvent> {
    j.seal(
        1_700_000_000_000,
        &Origin::Client,
        &[EventKind::AccountOpened {
            balance: 10_000_000,
        }],
    )
}

fn rec(id: &str) -> OutboxRecord {
    OutboxRecord {
        event_id: id.into(),
        routing_key: "terminal.trade.executed".into(),
        payload: format!("{{\"event_id\":\"{id}\"}}"),
    }
}

async fn store_with(ids: &[&str]) -> Arc<MemoryStore> {
    let store = Arc::new(MemoryStore::new());
    let j = Journal::new(1, 10_000_000);
    let out: Vec<OutboxRecord> = ids.iter().map(|i| rec(i)).collect();
    store
        .save_account(U, &snap(), &opening(&j), &out)
        .await
        .expect("запись");
    store
}

#[tokio::test]
async fn a_working_bus_empties_the_queue() {
    let store = store_with(&["e-1", "e-2"]).await;
    let pub_ = Flaky::new(0);
    let sent = relay_once(store.as_ref(), &pub_, 10).await.expect("проход");
    assert_eq!(sent, 2);
    assert_eq!(store.outbox_backlog().await.unwrap(), 0);
    let accepted = pub_.accepted.lock().unwrap().clone();
    assert_eq!(accepted.len(), 2);
    assert_eq!(
        accepted[0],
        ("terminal.trade.executed".into(), "e-1".into()),
        "ключ маршрутизации и message_id уходят как есть"
    );
}

/// Шина лежит — событие **не теряется**: остаётся в очереди, с попыткой и причиной.
#[tokio::test]
async fn an_unavailable_bus_keeps_the_event() {
    let store = store_with(&["e-1"]).await;
    let pub_ = Flaky::new(1);
    let sent = relay_once(store.as_ref(), &pub_, 10).await.expect("проход");

    assert_eq!(sent, 0, "ничего не ушло");
    assert_eq!(
        store.outbox_backlog().await.unwrap(),
        1,
        "и ничего не потеряно"
    );
    let dump = store.outbox_dump();
    assert_eq!(dump[0].1, 1, "попытка записана в строку");
    assert_eq!(
        dump[0].3.as_deref(),
        Some("шина недоступна"),
        "и причина тоже"
    );
}

/// Шина вернулась — событие доходит. Это и есть «обязаны дойти, когда вернётся».
#[tokio::test]
async fn the_event_arrives_when_the_bus_comes_back() {
    let store = store_with(&["e-1"]).await;
    let pub_ = Flaky::new(1);

    // Проход с арендой 0: строка сразу снова доступна, и второй проход — это та же
    // повторная доставка, только без ожидания настоящего отступа.
    relay_once(store.as_ref(), &pub_, 10).await.unwrap();
    // Отступ после отказа настоящий (5 с), поэтому возвращаем строку к сроку руками:
    // проверяем ПОРЯДОК повторной доставки, а не умение теста ждать пять секунд.
    store.mark_failed(1, 1, 0, "шина недоступна").await.unwrap();

    let sent = relay_once(store.as_ref(), &pub_, 10).await.unwrap();
    assert_eq!(sent, 1, "вернувшаяся шина получает отложенное");
    assert_eq!(store.outbox_backlog().await.unwrap(), 0);
    assert_eq!(pub_.accepted.lock().unwrap()[0].1, "e-1");
}

/// Отказ на одной строке **останавливает пачку**. Иначе одна чужая неудача увела
/// бы отступ остальных в часы — при том что причина у них общая и ещё не изучена.
#[tokio::test]
async fn one_failure_does_not_punish_the_rest_of_the_batch() {
    let store = store_with(&["e-1", "e-2", "e-3"]).await;
    let pub_ = Flaky::new(1);
    relay_once(store.as_ref(), &pub_, 10).await.unwrap();

    let dump = store.outbox_dump();
    assert_eq!(dump[0].1, 1, "отказавшая строка получила попытку");
    assert_eq!(dump[1].1, 0, "соседние — нет");
    assert_eq!(dump[2].1, 0);
    assert_eq!(
        store.outbox_backlog().await.unwrap(),
        3,
        "и все три остаются в очереди"
    );
}

/// Недоступное хранилище — отказ прохода, а не «пустая очередь».
/// Молчаливый `Ok(0)` здесь означал бы «слать нечего» при «не смог спросить».
#[tokio::test]
async fn an_unreadable_queue_is_a_failure_not_an_empty_queue() {
    struct NoQueue;
    #[async_trait::async_trait]
    impl Persistence for NoQueue {
        async fn init(&self) -> Result<(), gateway::persistence::StoreError> {
            Ok(())
        }
        async fn load_all(
            &self,
        ) -> Result<gateway::persistence::LoadedState, gateway::persistence::StoreError> {
            Ok(Default::default())
        }
        async fn save_account(
            &self,
            _u: UserId,
            _s: &AccountSnapshot,
            _e: &[broker::journal::AccountEvent],
            _o: &[OutboxRecord],
        ) -> Result<(), gateway::persistence::StoreError> {
            Ok(())
        }
    }
    let pub_ = Flaky::new(0);
    assert!(
        relay_once(&NoQueue, &pub_, 10).await.is_err(),
        "непрочитанная очередь обязана быть отказом"
    );
}
