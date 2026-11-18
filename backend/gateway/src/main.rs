#[tokio::main]
async fn main() {
    let mut state = match std::env::var("DATABASE_URL") {
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
    if let Err(e) = gateway::restore_state(&state).await {
        panic!("[gateway] не удалось поднять состояние: {e}");
    }
    gateway::announce_account_mode(&state);
    gateway::spawn_bus_relay(&mut state);
    gateway::feed::spawn(state.clone());
    gateway::spawn_monitor(state.clone());

    let app = gateway::router(state);
    let addr = "0.0.0.0:8080";
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    println!("[gateway] слушает http://{addr}  (реальные крипто-данные Binance, брокер+монитор)");
    axum::serve(listener, app).await.expect("serve");
}
