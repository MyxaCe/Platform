//! Бинарь шлюза: состояние + реальный крипто-фид Binance + слушает :8080.

#[tokio::main]
async fn main() {
    let state = gateway::build_state();
    gateway::seed_demo(&state).await;       // фундамент матчинга (фон, для /orders)
    gateway::feed::spawn(state.clone());    // реальные данные с Binance (ADR-013)

    let app = gateway::router(state);
    let addr = "0.0.0.0:8080";
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    println!("[gateway] слушает http://{addr}  (реальные крипто-данные Binance, топ-30)");
    axum::serve(listener, app).await.expect("serve");
}
