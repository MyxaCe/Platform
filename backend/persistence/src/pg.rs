//! PostgreSQL-реализация [`Persistence`] (ADR-016).
//!
//! Запросы runtime-проверяемые (`query`/`query_as`), **без макросов `query!`**: макросы
//! требуют живую БД на этапе компиляции, что сломало бы сборку образа в Docker.
//!
//! Деньги — `NUMERIC(38,0)`: вмещает весь диапазон `i128` и остаётся полноценным числом
//! для SQL (в отличие от текста), поэтому по деньгам можно считать инварианты запросом.

use async_trait::async_trait;
use bigdecimal::BigDecimal;
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{Pool, Postgres, Row};

use broker::journal::{AccountEvent, EventKind, Operator, Origin};
use broker::{AccountSnapshot, ClosedDeal, PendingOrder, PosSide, Position};
use domain::account::UserId;

use crate::money::{from_numeric, to_numeric};
use crate::{LoadedState, Persistence, StoreError};

fn db(e: sqlx::Error) -> StoreError {
    StoreError(e.to_string())
}

const SIDE_LONG: i16 = 0;
const SIDE_SHORT: i16 = 1;

fn side_to_db(s: PosSide) -> i16 {
    match s {
        PosSide::Long => SIDE_LONG,
        PosSide::Short => SIDE_SHORT,
    }
}
/// Модель позиций ↔ БД. Число, а не строка: строка на границе денег уже стоила нам
/// шести исправленных мест (TRM-07), и неизвестное значение обязано быть отказом, а
/// не молчаливым умолчанием.
fn model_to_db(m: broker::PositionModel) -> i16 {
    match m {
        broker::PositionModel::Hedging => 0,
        broker::PositionModel::Netting => 1,
    }
}
fn model_from_db(v: i16) -> broker::PositionModel {
    match v {
        1 => broker::PositionModel::Netting,
        0 => broker::PositionModel::Hedging,
        other => {
            // Неизвестное значение не подставляем молча: пишем и берём хеджирование
            // как исторически существовавшую модель. Инварианты счёта проверит
            // gateway; сюда попасть можно только правкой БД руками.
            eprintln!("[persistence] неизвестная модель позиций в БД: {other}, читаем как hedging");
            broker::PositionModel::Hedging
        }
    }
}

fn side_from_db(v: i16) -> PosSide {
    if v == SIDE_SHORT {
        PosSide::Short
    } else {
        PosSide::Long
    }
}

pub struct PgStore {
    pool: Pool<Postgres>,
}

impl PgStore {
    /// Подключиться к БД. `url` — строка вида `postgres://user:pass@host/db`.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect(url)
            .await
            .map_err(db)?;
        Ok(Self { pool })
    }
}

/// Идемпотентный DDL. Полноценные версионируемые миграции появятся при первом
/// изменении схемы на живых данных (ADR-016).
///
/// После пивота на автономный терминал (2026-07-25) учётные записи/сессии/passkey
/// ушли — счёт хранится сам по себе, `user_id` больше не ссылается на `users`.
const SCHEMA: &str = r#"
-- Внешняя личность SSO (tenant, sub) → внутренний user_id (ADR-023, Т2).
-- Последовательность стартует с 1000, чтобы не пересечься с легаси-счетами.
CREATE SEQUENCE IF NOT EXISTS identity_seq START 1000;
CREATE TABLE IF NOT EXISTS identities (
  user_id     BIGINT PRIMARY KEY DEFAULT nextval('identity_seq'),
  tenant      TEXT NOT NULL,
  sub         TEXT NOT NULL,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant, sub)
);
CREATE TABLE IF NOT EXISTS accounts (
  user_id     BIGINT PRIMARY KEY,
  balance     NUMERIC(38,0) NOT NULL,
  next_id     BIGINT NOT NULL
);
-- Модель ведения позиций счёта (ADR-025): 0 — хеджирование, 1 — неттинг.
-- Отдельным ALTER, а не полем в CREATE: у существующих баз таблица уже есть, и
-- CREATE TABLE IF NOT EXISTS её не тронет. DEFAULT 0 — не догадка: до 2026-09-04
-- другой модели в коде не существовало, значит все прежние счета хеджирующие.
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS model SMALLINT NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS positions (
  user_id     BIGINT NOT NULL,
  id          BIGINT NOT NULL,
  instrument  INTEGER NOT NULL,
  side        SMALLINT NOT NULL,
  qty         BIGINT NOT NULL,
  entry       BIGINT NOT NULL,
  pd          SMALLINT NOT NULL,
  qd          SMALLINT NOT NULL,
  margin      NUMERIC(38,0) NOT NULL,
  sl          BIGINT,
  tp          BIGINT,
  PRIMARY KEY (user_id, id)
);
CREATE TABLE IF NOT EXISTS pendings (
  user_id     BIGINT NOT NULL,
  id          BIGINT NOT NULL,
  instrument  INTEGER NOT NULL,
  side        SMALLINT NOT NULL,
  qty         BIGINT NOT NULL,
  price       BIGINT NOT NULL,
  pd          SMALLINT NOT NULL,
  qd          SMALLINT NOT NULL,
  sl          BIGINT,
  tp          BIGINT,
  PRIMARY KEY (user_id, id)
);
CREATE TABLE IF NOT EXISTS closed_deals (
  user_id     BIGINT NOT NULL,
  idx         BIGINT NOT NULL,
  instrument  INTEGER NOT NULL,
  side        SMALLINT NOT NULL,
  qty         BIGINT NOT NULL,
  entry       BIGINT NOT NULL,
  exit        BIGINT NOT NULL,
  pnl         NUMERIC(38,0) NOT NULL,
  pd          SMALLINT NOT NULL,
  qd          SMALLINT NOT NULL,
  closed_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, idx)
);
CREATE INDEX IF NOT EXISTS closed_deals_user_idx ON closed_deals (user_id, idx DESC);
-- Проскальзывание наружу (TRM-38). `exit` — цена ИСПОЛНЕНИЯ, `level` — уровень,
-- который ставил человек; их разница и есть проскальзывание. Отдельным ALTER,
-- а не полем в CREATE: у существующих баз таблица уже есть.
--
-- ОБА СТОЛБЦА NULLABLE И БЕЗ DEFAULT — это не недосмотр. У сделок, закрытых до
-- этой правки, мы НЕ ЗНАЕМ ни уровня, ни было ли закрытие на разрыве. `DEFAULT
-- false` для `across_gap` утверждал бы, что разрыва не было, — то есть отвечал
-- бы за прошлое, которого не видел. NULL означает «неизвестно», и читатель
-- обязан отличать его от «нет».
ALTER TABLE closed_deals ADD COLUMN IF NOT EXISTS level BIGINT;
ALTER TABLE closed_deals ADD COLUMN IF NOT EXISTS across_gap BOOLEAN;

-- ===================== Журнал событий счёта (TRM-13) ========================
-- Лента пополняется только добавлением. `kind` хранится ровно той канонической
-- строкой, по которой считался отпечаток: «что подписано» и «что лежит в базе» —
-- буквально одно и то же, и round-trip проверяется тестом без живой БД.
-- Отпечатки — 32 hex-символа (128 бит), TEXT: в NUMERIC они не нужны, по ним не
-- считают, их сравнивают.
CREATE TABLE IF NOT EXISTS account_events (
  user_id     BIGINT NOT NULL,
  seq         BIGINT NOT NULL,
  at_ms       BIGINT NOT NULL,
  origin      TEXT NOT NULL,
  who         TEXT,
  reason      TEXT,
  incident    TEXT,
  kind        TEXT NOT NULL,
  prev_hash   TEXT NOT NULL,
  hash        TEXT NOT NULL,
  PRIMARY KEY (user_id, seq)
);
-- Неизменяемость на уровне БД, а не на честном слове приложения: править журнал не
-- может и тот код, который в него пишет (Р-049). Образец — журнал аудита CMS.
CREATE OR REPLACE FUNCTION account_events_append_only() RETURNS trigger AS $fn$
BEGIN
  RAISE EXCEPTION 'account_events is append-only: % denied', TG_OP;
END;
$fn$ LANGUAGE plpgsql;
DROP TRIGGER IF EXISTS account_events_no_update ON account_events;
CREATE TRIGGER account_events_no_update
  BEFORE UPDATE OR DELETE ON account_events
  FOR EACH ROW EXECUTE FUNCTION account_events_append_only();
"#;

/// Отпечаток ↔ БД: 32 hex-символа. Число в тексте, а не `NUMERIC`, потому что по нему
/// не считают — его сравнивают, и сравнивать надо ровно те байты, что записали.
fn hash_to_db(h: u128) -> String {
    format!("{h:032x}")
}
fn hash_from_db(s: &str) -> Result<u128, StoreError> {
    u128::from_str_radix(s, 16)
        .map_err(|_| StoreError(format!("журнал: отпечаток «{s}» не читается")))
}

/// Поднять запись журнала из строки БД.
///
/// Отпечаток берётся как есть и **не пересчитывается**: иначе правка строки в базе
/// молча получила бы сходящийся отпечаток и цепочка перестала бы что-либо значить.
/// Сверяет цепочку вызывающий, при подъёме журнала.
fn event_from_row(r: &PgRow) -> Result<AccountEvent, StoreError> {
    let seq = r.get::<i64, _>("seq") as u64;
    let bad = |what: &str| StoreError(format!("журнал: запись №{seq} не читается ({what})"));
    let origin = match r.get::<String, _>("origin").as_str() {
        "system" => Origin::System,
        "client" => Origin::Client,
        "operator" => {
            // Пустое основание или потерянный инцидент — отказ чтения, а не «ну и
            // ладно»: ручная проводка без причины не отличима от злоупотребления.
            let who = r.get::<Option<String>, _>("who").unwrap_or_default();
            let reason = r.get::<Option<String>, _>("reason").unwrap_or_default();
            let incident = r.get::<Option<String>, _>("incident").unwrap_or_default();
            Origin::Operator(
                Operator::new(&who, &reason, &incident).map_err(|e| bad(&e.to_string()))?,
            )
        }
        other => return Err(bad(&format!("неизвестный источник «{other}»"))),
    };
    let kind = EventKind::decode(&r.get::<String, _>("kind")).map_err(|e| bad(&e.to_string()))?;
    let prev = hash_from_db(&r.get::<String, _>("prev_hash"))?;
    let hash = hash_from_db(&r.get::<String, _>("hash"))?;
    Ok(AccountEvent::from_storage(
        seq,
        r.get::<i64, _>("at_ms"),
        origin,
        kind,
        prev,
        hash,
    ))
}

#[async_trait]
impl Persistence for PgStore {
    async fn init(&self) -> Result<(), StoreError> {
        // Несколько выражений одним execute: sqlx выполнит их как batch.
        sqlx::raw_sql(SCHEMA)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn load_all(&self) -> Result<LoadedState, StoreError> {
        // Счёт — самостоятельная сущность: перебираем то, что реально лежит в accounts.
        let acc_rows =
            sqlx::query("SELECT user_id, balance, next_id, model FROM accounts ORDER BY user_id")
                .fetch_all(&self.pool)
                .await
                .map_err(db)?;

        let mut accounts = Vec::new();
        for acc in &acc_rows {
            let uid = UserId(acc.get::<i64, _>("user_id") as u64);
            let id_db = uid.0 as i64;
            let balance = from_numeric(&acc.get::<BigDecimal, _>("balance"))?;
            let next_id = acc.get::<i64, _>("next_id") as u64;

            let model = model_from_db(acc.get::<i16, _>("model"));

            let positions = sqlx::query(
                "SELECT id, instrument, side, qty, entry, pd, qd, margin, sl, tp FROM positions WHERE user_id = $1 ORDER BY id",
            )
            .bind(id_db)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .iter()
            .map(|r: &PgRow| {
                Ok(Position {
                    id: r.get::<i64, _>("id") as u64,
                    instrument: r.get::<i32, _>("instrument") as u32,
                    side: side_from_db(r.get::<i16, _>("side")),
                    qty: r.get::<i64, _>("qty"),
                    entry: r.get::<i64, _>("entry"),
                    pd: r.get::<i16, _>("pd") as u8,
                    qd: r.get::<i16, _>("qd") as u8,
                    margin: from_numeric(&r.get::<BigDecimal, _>("margin"))?,
                    sl: r.get::<Option<i64>, _>("sl"),
                    tp: r.get::<Option<i64>, _>("tp"),
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;

            let pendings = sqlx::query(
                "SELECT id, instrument, side, qty, price, pd, qd, sl, tp FROM pendings WHERE user_id = $1 ORDER BY id",
            )
            .bind(id_db)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .iter()
            .map(|r: &PgRow| PendingOrder {
                id: r.get::<i64, _>("id") as u64,
                instrument: r.get::<i32, _>("instrument") as u32,
                side: side_from_db(r.get::<i16, _>("side")),
                qty: r.get::<i64, _>("qty"),
                price: r.get::<i64, _>("price"),
                pd: r.get::<i16, _>("pd") as u8,
                qd: r.get::<i16, _>("qd") as u8,
                sl: r.get::<Option<i64>, _>("sl"),
                tp: r.get::<Option<i64>, _>("tp"),
            })
            .collect();

            let closed = sqlx::query(
                "SELECT instrument, side, qty, entry, exit, pnl, pd, qd, level, across_gap FROM closed_deals WHERE user_id = $1 ORDER BY idx",
            )
            .bind(id_db)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .iter()
            .map(|r: &PgRow| {
                Ok(ClosedDeal {
                    instrument: r.get::<i32, _>("instrument") as u32,
                    side: side_from_db(r.get::<i16, _>("side")),
                    qty: r.get::<i64, _>("qty"),
                    entry: r.get::<i64, _>("entry"),
                    exit: r.get::<i64, _>("exit"),
                    pnl: from_numeric(&r.get::<BigDecimal, _>("pnl"))?,
                    pd: r.get::<i16, _>("pd") as u8,
                    qd: r.get::<i16, _>("qd") as u8,
                    // NULL здесь означает «неизвестно», а не «нет»: так лежат
                    // сделки, закрытые до появления этих столбцов (TRM-38).
                    level: r.get::<Option<i64>, _>("level"),
                    across_gap: r.get::<Option<bool>, _>("across_gap"),
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;

            accounts.push((
                uid,
                AccountSnapshot {
                    balance,
                    next_id,
                    positions,
                    pendings,
                    closed,
                    model,
                },
            ));
        }

        // Журналы (TRM-13). Читаются по тем же счетам и в порядке записи: сверку
        // делает вызывающий, здесь только подъём без единой догадки — неразобранная
        // запись это отказ чтения, а не пропуск строки.
        let mut journals = Vec::new();
        for acc in &acc_rows {
            let uid = UserId(acc.get::<i64, _>("user_id") as u64);
            let rows = sqlx::query(
                "SELECT seq, at_ms, origin, who, reason, incident, kind, prev_hash, hash
                 FROM account_events WHERE user_id = $1 ORDER BY seq",
            )
            .bind(uid.0 as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
            let mut lane = Vec::with_capacity(rows.len());
            for r in &rows {
                lane.push(event_from_row(r)?);
            }
            if !lane.is_empty() {
                journals.push((uid, lane));
            }
        }

        Ok(LoadedState { accounts, journals })
    }

    async fn save_account(
        &self,
        user: UserId,
        snap: &AccountSnapshot,
        events: &[AccountEvent],
    ) -> Result<(), StoreError> {
        let id_db = user.0 as i64;
        let mut tx = self.pool.begin().await.map_err(db)?;

        sqlx::query(
            "INSERT INTO accounts (user_id, balance, next_id, model) VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id) DO UPDATE SET balance = EXCLUDED.balance, next_id = EXCLUDED.next_id, model = EXCLUDED.model",
        )
        .bind(id_db)
        .bind(to_numeric(snap.balance))
        .bind(snap.next_id as i64)
        .bind(model_to_db(snap.model))
        .execute(&mut *tx)
        .await
        .map_err(db)?;

        // Позиции и ордера переписываются целиком: их единицы-десятки, и это избавляет
        // от диффа. История так себя вести не может — она только дописывается.
        sqlx::query("DELETE FROM positions WHERE user_id = $1")
            .bind(id_db)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        for p in &snap.positions {
            sqlx::query(
                "INSERT INTO positions (user_id, id, instrument, side, qty, entry, pd, qd, margin, sl, tp)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
            )
            .bind(id_db).bind(p.id as i64).bind(p.instrument as i32).bind(side_to_db(p.side))
            .bind(p.qty).bind(p.entry).bind(p.pd as i16).bind(p.qd as i16)
            .bind(to_numeric(p.margin)).bind(p.sl).bind(p.tp)
            .execute(&mut *tx).await.map_err(db)?;
        }

        sqlx::query("DELETE FROM pendings WHERE user_id = $1")
            .bind(id_db)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        for p in &snap.pendings {
            sqlx::query(
                "INSERT INTO pendings (user_id, id, instrument, side, qty, price, pd, qd, sl, tp)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(id_db)
            .bind(p.id as i64)
            .bind(p.instrument as i32)
            .bind(side_to_db(p.side))
            .bind(p.qty)
            .bind(p.price)
            .bind(p.pd as i16)
            .bind(p.qd as i16)
            .bind(p.sl)
            .bind(p.tp)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }

        // Дописываем только хвост истории: стоимость закрытия сделки не должна зависеть
        // от её длины. Сколько уже лежит — спрашиваем в той же транзакции.
        let have: i64 = sqlx::query("SELECT count(*) AS n FROM closed_deals WHERE user_id = $1")
            .bind(id_db)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?
            .get("n");
        for (i, d) in snap.closed.iter().enumerate().skip(have as usize) {
            sqlx::query(
                "INSERT INTO closed_deals (user_id, idx, instrument, side, qty, entry, exit, pnl, pd, qd, level, across_gap)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT (user_id, idx) DO NOTHING",
            )
            .bind(id_db).bind(i as i64).bind(d.instrument as i32).bind(side_to_db(d.side))
            .bind(d.qty).bind(d.entry).bind(d.exit).bind(to_numeric(d.pnl))
            .bind(d.pd as i16).bind(d.qd as i16)
            .bind(d.level).bind(d.across_gap)
            .execute(&mut *tx).await.map_err(db)?;
        }

        // Журнал — в той же транзакции, что и слепок (TRM-13). Разъехаться им нельзя:
        // иначе падение между двумя записями произвело бы ровно то расхождение, ради
        // обнаружения которого журнал и заводился.
        //
        // Никакого ON CONFLICT: повтор номера обязан падать. Уникальный ключ здесь —
        // это «поверх записанного не пишут», и глушить его значило бы терять запись.
        for e in events {
            let (who, reason, incident) = match e.origin().operator() {
                Some(o) => (Some(o.who()), Some(o.reason()), Some(o.incident())),
                None => (None, None, None),
            };
            sqlx::query(
                "INSERT INTO account_events (user_id, seq, at_ms, origin, who, reason, incident, kind, prev_hash, hash)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(id_db)
            .bind(e.seq() as i64)
            .bind(e.at_ms())
            .bind(e.origin().tag())
            .bind(who)
            .bind(reason)
            .bind(incident)
            .bind(e.kind().encode())
            .bind(hash_to_db(e.prev()))
            .bind(hash_to_db(e.hash()))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }

        tx.commit().await.map_err(db)?;
        Ok(())
    }

    fn durable(&self) -> bool {
        true
    }

    fn kind(&self) -> &'static str {
        "postgres"
    }

    async fn resolve_identity(&self, tenant: &str, sub: &str) -> Result<UserId, StoreError> {
        // Найти-или-создать одним запросом: конфликт по (tenant, sub) => RETURNING
        // существующего user_id (DO UPDATE фиктивный, чтобы RETURNING сработал).
        let row = sqlx::query(
            "INSERT INTO identities (tenant, sub) VALUES ($1, $2)
             ON CONFLICT (tenant, sub) DO UPDATE SET tenant = EXCLUDED.tenant
             RETURNING user_id",
        )
        .bind(tenant)
        .bind(sub)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        Ok(UserId(row.get::<i64, _>("user_id") as u64))
    }
}
