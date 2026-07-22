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
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
  id          BIGINT PRIMARY KEY,
  token       TEXT NOT NULL UNIQUE,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Учётные данные (ADR-018). Колонки добавляются отдельно: таблица users уже могла
-- быть создана прошлой версией. token становится необязательным — у пользователей,
-- зарегистрированных по почте, dev-токена нет.
ALTER TABLE users ADD COLUMN IF NOT EXISTS email TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS password_hash TEXT;
ALTER TABLE users ALTER COLUMN token DROP NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS users_email_uidx ON users (email);
CREATE TABLE IF NOT EXISTS sessions (
  token_hash  TEXT PRIMARY KEY,          -- SHA-256 от токена; сам токен не хранится
  user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at  BIGINT NOT NULL,           -- unix-секунды
  expires_at  BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_user_idx ON sessions (user_id);
CREATE TABLE IF NOT EXISTS passkeys (
  cred_id     TEXT PRIMARY KEY,          -- id учётных данных ключа (url-safe base64)
  user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  data        TEXT NOT NULL,             -- JSON публичного ключа (webauthn Passkey)
  created_at  BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS passkeys_user_idx ON passkeys (user_id);
CREATE TABLE IF NOT EXISTS accounts (
  user_id     BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  balance     NUMERIC(38,0) NOT NULL,
  next_id     BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS positions (
  user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
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
  user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
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
  user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
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
"#;

#[async_trait]
impl Persistence for PgStore {
    async fn init(&self) -> Result<(), StoreError> {
        // Несколько выражений одним execute: sqlx выполнит их как batch.
        sqlx::raw_sql(SCHEMA).execute(&self.pool).await.map_err(db)?;
        Ok(())
    }

    async fn load_all(&self) -> Result<LoadedState, StoreError> {
        let user_rows = sqlx::query("SELECT id, token, email, password_hash FROM users ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        let mut users: Vec<(UserId, String)> = Vec::new();
        let mut credentials = Vec::new();
        let mut ids: Vec<UserId> = Vec::new();
        for r in &user_rows {
            let id = UserId(r.get::<i64, _>("id") as u64);
            ids.push(id);
            if let Some(tok) = r.get::<Option<String>, _>("token") {
                users.push((id, tok));
            }
            if let (Some(email), Some(hash)) = (r.get::<Option<String>, _>("email"), r.get::<Option<String>, _>("password_hash")) {
                credentials.push((id, email, hash));
            }
        }

        let sessions = sqlx::query("SELECT token_hash, user_id, expires_at FROM sessions")
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .iter()
            .map(|r: &PgRow| (r.get::<String, _>("token_hash"), UserId(r.get::<i64, _>("user_id") as u64), r.get::<i64, _>("expires_at")))
            .collect();

        let passkeys = sqlx::query("SELECT user_id, data FROM passkeys")
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .iter()
            .map(|r: &PgRow| (UserId(r.get::<i64, _>("user_id") as u64), r.get::<String, _>("data")))
            .collect();

        let mut accounts = Vec::new();
        for uid in &ids {
            let id_db = uid.0 as i64;

            let acc = sqlx::query("SELECT balance, next_id FROM accounts WHERE user_id = $1")
                .bind(id_db)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
            let Some(acc) = acc else { continue };
            let balance = from_numeric(&acc.get::<BigDecimal, _>("balance"))?;
            let next_id = acc.get::<i64, _>("next_id") as u64;

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
                "SELECT instrument, side, qty, entry, exit, pnl, pd, qd FROM closed_deals WHERE user_id = $1 ORDER BY idx",
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
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;

            accounts.push((*uid, AccountSnapshot { balance, next_id, positions, pendings, closed }));
        }

        Ok(LoadedState { users, credentials, sessions, passkeys, accounts })
    }

    async fn save_account(&self, user: UserId, snap: &AccountSnapshot) -> Result<(), StoreError> {
        let id_db = user.0 as i64;
        let mut tx = self.pool.begin().await.map_err(db)?;

        // Счёт мог появиться раньше пользователя (демо-сид) — гарантируем строку users.
        sqlx::query("INSERT INTO users (id, token) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING")
            .bind(id_db)
            .bind(format!("user-{}", user.0))
            .execute(&mut *tx)
            .await
            .map_err(db)?;

        sqlx::query(
            "INSERT INTO accounts (user_id, balance, next_id) VALUES ($1, $2, $3)
             ON CONFLICT (user_id) DO UPDATE SET balance = EXCLUDED.balance, next_id = EXCLUDED.next_id",
        )
        .bind(id_db)
        .bind(to_numeric(snap.balance))
        .bind(snap.next_id as i64)
        .execute(&mut *tx)
        .await
        .map_err(db)?;

        // Позиции и ордера переписываются целиком: их единицы-десятки, и это избавляет
        // от диффа. История так себя вести не может — она только дописывается.
        sqlx::query("DELETE FROM positions WHERE user_id = $1").bind(id_db).execute(&mut *tx).await.map_err(db)?;
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

        sqlx::query("DELETE FROM pendings WHERE user_id = $1").bind(id_db).execute(&mut *tx).await.map_err(db)?;
        for p in &snap.pendings {
            sqlx::query(
                "INSERT INTO pendings (user_id, id, instrument, side, qty, price, pd, qd, sl, tp)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(id_db).bind(p.id as i64).bind(p.instrument as i32).bind(side_to_db(p.side))
            .bind(p.qty).bind(p.price).bind(p.pd as i16).bind(p.qd as i16)
            .bind(p.sl).bind(p.tp)
            .execute(&mut *tx).await.map_err(db)?;
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
                "INSERT INTO closed_deals (user_id, idx, instrument, side, qty, entry, exit, pnl, pd, qd)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (user_id, idx) DO NOTHING",
            )
            .bind(id_db).bind(i as i64).bind(d.instrument as i32).bind(side_to_db(d.side))
            .bind(d.qty).bind(d.entry).bind(d.exit).bind(to_numeric(d.pnl))
            .bind(d.pd as i16).bind(d.qd as i16)
            .execute(&mut *tx).await.map_err(db)?;
        }

        tx.commit().await.map_err(db)?;
        Ok(())
    }

    async fn save_user(&self, user: UserId, token: &str) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO users (id, token) VALUES ($1, $2)
             ON CONFLICT (id) DO UPDATE SET token = EXCLUDED.token",
        )
        .bind(user.0 as i64)
        .bind(token)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn save_credentials(&self, user: UserId, email: &str, password_hash: &str) -> Result<(), StoreError> {
        // Уникальность почты держит индекс: проверка «занята ли» в приложении не спасает
        // от гонки двух одновременных регистраций.
        sqlx::query(
            "INSERT INTO users (id, email, password_hash) VALUES ($1, $2, $3)
             ON CONFLICT (id) DO UPDATE SET email = EXCLUDED.email, password_hash = EXCLUDED.password_hash",
        )
        .bind(user.0 as i64)
        .bind(email)
        .bind(password_hash)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn save_session(&self, token_hash: &str, user: UserId, created: i64, expires: i64) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)
             ON CONFLICT (token_hash) DO NOTHING",
        )
        .bind(token_hash)
        .bind(user.0 as i64)
        .bind(created)
        .bind(expires)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn delete_session(&self, token_hash: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn save_passkey(&self, user: UserId, cred_id: &str, data: &str) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO passkeys (cred_id, user_id, data) VALUES ($1, $2, $3)
             ON CONFLICT (cred_id) DO UPDATE SET data = EXCLUDED.data",
        )
        .bind(cred_id)
        .bind(user.0 as i64)
        .bind(data)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn check_invariants(&self) -> Result<Vec<String>, StoreError> {
        // Красная линия №5. Наличие настоящих чисел в SQL (а не текста) — ровно то,
        // ради чего выбран NUMERIC вместо SQLite-текста.
        let mut broken = Vec::new();
        let neg: i64 = sqlx::query("SELECT count(*) AS n FROM accounts WHERE balance < 0")
            .fetch_one(&self.pool)
            .await
            .map_err(db)?
            .get("n");
        if neg > 0 {
            broken.push(format!("счетов с отрицательным балансом: {neg}"));
        }
        Ok(broken)
    }
}
