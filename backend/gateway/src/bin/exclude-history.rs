use broker::journal::{Journal, Operator};
use domain::account::UserId;
use gateway::exclusion;
use gateway::persistence::{Persistence, PgStore};

struct Args {
    user: u64,
    who: String,
    reason: String,
    incident: String,
    apply: bool,
}

fn parse() -> Result<Args, String> {
    let mut user = None;
    let (mut who, mut reason, mut incident) = (None, None, None);
    let mut apply = false;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("у «{flag}» не хватает значения"))
        };
        match flag.as_str() {
            "--user" => {
                let v = value()?;
                user = Some(
                    v.parse::<u64>()
                        .map_err(|_| format!("--user: «{v}» не число"))?,
                );
            }
            "--who" => who = Some(value()?),
            "--reason" => reason = Some(value()?),
            "--incident" => incident = Some(value()?),
            "--apply" => apply = true,
            other => return Err(format!("неизвестный аргумент «{other}»")),
        }
    }
    Ok(Args {
        user: user.ok_or("нужен --user")?,
        who: who.ok_or("нужен --who")?,
        reason: reason.ok_or("нужен --reason")?,
        incident: incident.ok_or("нужен --incident")?,
        apply,
    })
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("[exclude-history] {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = parse()?;
    let user = UserId(args.user);
    let operator = Operator::new(&args.who, &args.reason, &args.incident)
        .map_err(|e| format!("основание не принято: {e}"))?;

    let url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .ok_or("нужен DATABASE_URL: объявление пишется в журнал, а он в базе")?;
    let store = PgStore::connect(&url)
        .await
        .map_err(|e| format!("нет связи с БД: {e}"))?;

    let start_balance: i128 = std::env::var("DEMO_START_BALANCE_CENTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|c: &i128| *c > 0)
        .unwrap_or(10_000_000);

    let loaded = store
        .load_all()
        .await
        .map_err(|e| format!("не удалось прочитать состояние: {e}"))?;
    let snap = loaded
        .accounts
        .iter()
        .find(|(u, _)| *u == user)
        .map(|(_, s)| s.clone())
        .ok_or_else(|| format!("счёта {} в хранилище нет", user.0))?;
    let events = loaded
        .journals
        .into_iter()
        .find(|(u, _)| *u == user)
        .map(|(_, e)| e)
        .ok_or_else(|| format!("журнала счёта {} в хранилище нет", user.0))?;

    let journal = Journal::load(start_balance, events)
        .map_err(|e| format!("журнал не прошёл проверку, объявление не делается: {e}"))?;

    let at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let decl = exclusion::declare(&journal, at_ms, operator).map_err(|e| e.to_string())?;
    let record = &decl.sealed[0];

    println!("счёт:            {}", user.0);
    println!("записей в ленте: {}", journal.events().len());
    println!(
        "граница:         по запись №{} включительно",
        decl.through_seq
    );
    println!(
        "новая запись:    №{} — {}",
        record.seq(),
        record.kind().encode()
    );
    println!("кто:             {}", args.who);
    println!("основание:       {}", args.reason);
    println!("случай:          {}", args.incident);
    println!("остаток:         {} (не меняется)", snap.balance);

    if !args.apply {
        println!();
        println!("ПРОБНЫЙ ПРОГОН: ничего не записано. Повторить с --apply.");
        return Ok(());
    }

    store
        .save_account(user, &snap, &decl.sealed, &[])
        .await
        .map_err(|e| format!("запись не удалась, журнал не тронут: {e}"))?;

    println!();
    println!("ЗАПИСАНО. Лента продолжена записью №{}.", record.seq());
    Ok(())
}
