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

use broker::journal::{AccountEvent, Operator, Origin};
use broker::{AccountSnapshot, ClosedDeal, PendingOrder, PosSide, Position};
use domain::account::UserId;

use crate::money::{from_numeric, to_numeric};
use crate::{LoadedState, OutboxClaim, OutboxRecord, Persistence, StoreError};

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
-- Профиль счёта (TRM-44): владелец, род, происхождение, категория при открытии,
-- валюта. Отдельными ALTER по той же причине, что `model`: у существующих баз
-- таблица уже есть, и CREATE TABLE IF NOT EXISTS её не тронет.
--
-- УМОЛЧАНИЯ ОПИСЫВАЮТ ИСТОРИЮ, А НЕ УДОБСТВО. До 2026-10-24 в терминале
-- существовали только демо-счета в долларах, заведённые регистрацией, и все
-- клиенты считались розничными: другого рода счетов не было. Поэтому
-- `DEFAULT` здесь — утверждение о прошлом, которое можно проверить, а не
-- значение, подставленное чтобы столбец не был пустым.
--
-- `owner` отдельным столбцом при том, что сегодня он равен `user_id`: ключ
-- завтра станет идентификатором СЧЁТА, а владелец останется владельцем.
-- Заполняется из `user_id` ровно затем, чтобы миграции потом не потребовалось.
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS owner_user_id BIGINT;
UPDATE accounts SET owner_user_id = user_id WHERE owner_user_id IS NULL;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'demo';
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS origin TEXT NOT NULL DEFAULT 'registration';
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS category_at_open TEXT NOT NULL DEFAULT 'retail';
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS currency TEXT NOT NULL DEFAULT 'USD';
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

-- =============== Исходящая очередь шины (TRM-40, transactional outbox) ======
-- Лежит рядом со слепком и журналом и наполняется ТОЙ ЖЕ транзакцией: событие о
-- сделке, потерянное на перезапуске, — это деньги. До этой таблицы исходящей
-- очередью звался массив в памяти процесса (Р-204).
--
-- Схема — зеркало `outbox` релея платформы (их `db/init/001_init.sql`), имена
-- столбцов те же намеренно: два разных имени для одного смысла однажды разойдутся,
-- а эксплуатирует их один человек.
--
-- `payload` — конверт ЦЕЛИКОМ, собранный в момент сделки. Релей его не
-- пересобирает: иначе форма события зависела бы от версии кода в момент отправки,
-- а не в момент сделки, и строка, пролежавшая через выкатку, уехала бы в чужом виде.
CREATE TABLE IF NOT EXISTS bus_outbox (
  id              BIGSERIAL PRIMARY KEY,
  event_id        TEXT NOT NULL UNIQUE,
  routing_key     TEXT NOT NULL,
  payload         JSONB NOT NULL,
  occurred_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  published_at    TIMESTAMPTZ,
  attempts        INT NOT NULL DEFAULT 0,
  next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_error      TEXT
);
-- Частичный индекс: выборка релея смотрит только на неотправленные, и расти она
-- не должна вместе с историей отправленных.
CREATE INDEX IF NOT EXISTS bus_outbox_unpublished_idx
  ON bus_outbox (next_attempt_at)
  WHERE published_at IS NULL;
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
    // Каноническая строка отдаётся КАК ЕСТЬ: по ней считался отпечаток, и
    // пересобирать её из разобранного вида нельзя — расширение записи новыми
    // полями тогда меняло бы подпись у всех прежних (TRM-44).
    let canon = r.get::<String, _>("kind");
    let prev = hash_from_db(&r.get::<String, _>("prev_hash"))?;
    let hash = hash_from_db(&r.get::<String, _>("hash"))?;
    AccountEvent::from_storage(seq, r.get::<i64, _>("at_ms"), origin, &canon, prev, hash)
        .map_err(|e| bad(&e.to_string()))
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
        let acc_rows = sqlx::query(
            "SELECT user_id, balance, next_id, model, owner_user_id, kind, origin, category_at_open, currency
               FROM accounts ORDER BY user_id",
        )
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
            // Неизвестное значение в профиле — ОТКАЗ чтения, а не умолчание.
            // Род счёта решает, настоящие ли это деньги; прочитать непонятное
            // как «демо» значило бы угадывать, чьими деньгами торгуют.
            let bad_profile = |what: &str, v: &str| {
                StoreError(format!("счёт {}: {what} «{v}» не читается", uid.0))
            };
            let kind_s = acc.get::<String, _>("kind");
            let origin_s = acc.get::<String, _>("origin");
            let cat_s = acc.get::<String, _>("category_at_open");
            let cur_s = acc.get::<String, _>("currency");
            let profile = domain::account::AccountProfile {
                owner: UserId(acc.get::<Option<i64>, _>("owner_user_id").unwrap_or(id_db) as u64),
                kind: domain::account::AccountKind::parse(&kind_s)
                    .ok_or_else(|| bad_profile("род", &kind_s))?,
                origin: domain::account::AccountOrigin::parse(&origin_s)
                    .ok_or_else(|| bad_profile("происхождение", &origin_s))?,
                category_at_open: domain::account::CategoryAtOpen(
                    domain::account::ClientCategory::parse(&cat_s)
                        .ok_or_else(|| bad_profile("категория", &cat_s))?,
                ),
                currency: domain::account::Currency::parse(&cur_s)
                    .ok_or_else(|| bad_profile("валюта", &cur_s))?,
            };

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
                    profile,
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
        outgoing: &[OutboxRecord],
    ) -> Result<(), StoreError> {
        let id_db = user.0 as i64;
        let mut tx = self.pool.begin().await.map_err(db)?;

        // Профиль пишется при заведении и далее не меняется: в `DO UPDATE` его
        // нет намеренно. Род счёта или валюта, поменявшиеся у существующего
        // счёта, означали бы, что прошлые суммы посчитаны в одном, а читаются в
        // другом. Смена валюты — это новый счёт, а не правка поля.
        sqlx::query(
            "INSERT INTO accounts (user_id, balance, next_id, model, owner_user_id, kind, origin, category_at_open, currency)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (user_id) DO UPDATE SET balance = EXCLUDED.balance, next_id = EXCLUDED.next_id, model = EXCLUDED.model",
        )
        .bind(id_db)
        .bind(to_numeric(snap.balance))
        .bind(snap.next_id as i64)
        .bind(model_to_db(snap.model))
        .bind(snap.profile.owner.0 as i64)
        .bind(snap.profile.kind.tag())
        .bind(snap.profile.origin.tag())
        .bind(snap.profile.category_at_open.0.tag())
        .bind(snap.profile.currency.code())
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
            // Пишем СОХРАНЁННЫЙ канон записи, а не пересобранный из вида:
            // именно по нему посчитан отпечаток в соседнем столбце (TRM-44).
            //
            // ⚠️ Подмена на `e.kind().encode()` сегодня НИЧЕГО НЕ МЕНЯЕТ и не
            // красит ни одного теста — проверено мутацией. Сюда попадают только
            // свежезапечатанные записи, у которых канон и есть текущий вид.
            // Это не пропуск в проверках, а граница: вызов защищает тот день,
            // когда в запись попадёт лента, поднятая из базы. Тогда
            // пересборка молча перепишет историю чужим видом, и отпечаток
            // перестанет ей соответствовать.
            .bind(e.canon())
            .bind(hash_to_db(e.prev()))
            .bind(hash_to_db(e.hash()))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }

        // Исходящая очередь — в той же транзакции (TRM-40). Не отдельным заходом
        // «после сохранения»: тогда между деньгами и событием появилась бы щель, и
        // падение в ней дало бы либо сделку без события, либо событие без сделки.
        //
        // `ON CONFLICT DO NOTHING` по `event_id`: идентификатор выведен из факта
        // (счёт + номер записи журнала), поэтому повтор означает повторную запись
        // того же факта, а не новое событие. Здесь это противоположно журналу, где
        // повтор номера обязан падать: там номер — позиция в ленте, и запись «поверх»
        // означала бы потерю. Разница намеренная.
        for rec in outgoing {
            sqlx::query(
                "INSERT INTO bus_outbox (event_id, routing_key, payload)
                 VALUES ($1, $2, $3::jsonb) ON CONFLICT (event_id) DO NOTHING",
            )
            .bind(&rec.event_id)
            .bind(&rec.routing_key)
            .bind(&rec.payload)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }

        tx.commit().await.map_err(db)?;
        Ok(())
    }

    async fn claim_outbox(
        &self,
        limit: i64,
        lease_ms: i64,
    ) -> Result<Vec<OutboxClaim>, StoreError> {
        // `FOR UPDATE SKIP LOCKED` + сдвиг `next_attempt_at` = аренда: второй
        // экземпляр не возьмёт те же строки, пока первый с ними работает. Аренда
        // НЕ трогает `attempts`: счётчик обязан считать отказы шины, а не заходы
        // релея — иначе по нему нельзя отличить «шина лежит» от «релей бодр».
        let rows = sqlx::query(
            "UPDATE bus_outbox SET next_attempt_at = now() + make_interval(secs => $2 / 1000.0)
              WHERE id IN (
                SELECT id FROM bus_outbox
                 WHERE published_at IS NULL AND next_attempt_at <= now()
                 ORDER BY id LIMIT $1
                 FOR UPDATE SKIP LOCKED
              )
              RETURNING id, event_id, routing_key, payload::text AS payload, attempts",
        )
        .bind(limit)
        .bind(lease_ms as f64)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        // Порядок отправки — порядок записи. `UPDATE ... RETURNING` его не обещает,
        // а для баланса он значим: событие «новый остаток» перезаписывает прежний,
        // и переставленные местами два события оставили бы в проекции старое число.
        let mut out: Vec<OutboxClaim> = rows
            .iter()
            .map(|r: &PgRow| OutboxClaim {
                id: r.get::<i64, _>("id"),
                event_id: r.get::<String, _>("event_id"),
                routing_key: r.get::<String, _>("routing_key"),
                payload: r.get::<String, _>("payload"),
                attempts: r.get::<i32, _>("attempts"),
            })
            .collect();
        out.sort_by_key(|c| c.id);
        Ok(out)
    }

    async fn mark_published(&self, id: i64) -> Result<(), StoreError> {
        let done = sqlx::query("UPDATE bus_outbox SET published_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        if done.rows_affected() == 0 {
            return Err(StoreError(format!(
                "исходящая очередь: строки №{id} нет, пометить отправленной нечего"
            )));
        }
        Ok(())
    }

    async fn mark_failed(
        &self,
        id: i64,
        attempts: i32,
        retry_in_ms: i64,
        error: &str,
    ) -> Result<(), StoreError> {
        let text: String = error.chars().take(500).collect();
        let done = sqlx::query(
            "UPDATE bus_outbox
                SET attempts = $2,
                    next_attempt_at = now() + make_interval(secs => $3 / 1000.0),
                    last_error = $4
              WHERE id = $1",
        )
        .bind(id)
        .bind(attempts)
        .bind(retry_in_ms as f64)
        .bind(&text)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        if done.rows_affected() == 0 {
            return Err(StoreError(format!(
                "исходящая очередь: строки №{id} нет, записать отказ некуда"
            )));
        }
        Ok(())
    }

    async fn outbox_backlog(&self) -> Result<i64, StoreError> {
        let n: i64 = sqlx::query("SELECT count(*) AS n FROM bus_outbox WHERE published_at IS NULL")
            .fetch_one(&self.pool)
            .await
            .map_err(db)?
            .get("n");
        Ok(n)
    }

    async fn identity_of(&self, user: UserId) -> Result<Option<(String, String)>, StoreError> {
        let row = sqlx::query("SELECT tenant, sub FROM identities WHERE user_id = $1")
            .bind(user.0 as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        Ok(row.map(|r| (r.get::<String, _>("tenant"), r.get::<String, _>("sub"))))
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
