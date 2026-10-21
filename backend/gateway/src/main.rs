//! Бинарь шлюза: состояние + реальный крипто-фид Binance + монитор брокера + слушает :8080.

#[tokio::main]
async fn main() {
    // Хранилище выбирается переменной окружения (ADR-016). Без неё — MemoryStore:
    // всё в памяти процесса, чтобы локальный запуск и тесты не требовали БД. Путь
    // записи при этом настоящий, включая журнал счёта (ADR-027).
    let state = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => {
            let store = gateway::persistence::PgStore::connect(&url)
                .await
                .unwrap_or_else(|e| panic!("[gateway] нет связи с БД: {e}"));
            println!("[gateway] хранилище: PostgreSQL");
            gateway::build_state_with(std::sync::Arc::new(store))
        }
        _ => {
            println!("[gateway] хранилище: нет (DATABASE_URL не задан) — состояние живёт только в памяти");
            gateway::build_state()
        }
    };
    // Поднять состояние и журналы счетов до приёма запросов (ADR-016, ADR-027).
    // Без БД — пусто. Ошибка здесь фатальна: работать на неизвестном состоянии
    // счетов и непрочитанной их истории нельзя (красные линии №3 и №5).
    if let Err(e) = gateway::restore_state(&state).await {
        panic!("[gateway] не удалось поднять состояние: {e}");
    }
    gateway::feed::spawn(state.clone()); // реальные данные с Binance (ADR-013)
    gateway::spawn_monitor(state.clone()); // триггеры лимитных ордеров и SL/TP (ADR-014)

    let app = gateway::router(state);
    let addr = "0.0.0.0:8080";
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    println!("[gateway] слушает http://{addr}  (реальные крипто-данные Binance, брокер+монитор)");
    axum::serve(listener, app).await.expect("serve");
}
