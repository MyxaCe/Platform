//! Бинарь шлюза: состояние + реальный крипто-фид Binance + монитор брокера + слушает :8080.

#[tokio::main]
async fn main() {
    let state = gateway::build_state();
    gateway::seed_demo(&state).await;       // фундамент матчинга (фон, для /orders)
    gateway::feed::spawn(state.clone());    // реальные данные с Binance (ADR-013)
    gateway::spawn_monitor(state.clone());  // триггеры лимитных ордеров и SL/TP (ADR-014)

    let app = gateway::router(state);
    let addr = "0.0.0.0:8080";
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    println!("[gateway] слушает http://{addr}  (реальные крипто-данные Binance, брокер+монитор)");
    axum::serve(listener, app).await.expect("serve");
}
