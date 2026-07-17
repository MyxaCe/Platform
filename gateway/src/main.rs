//! Бинарь шлюза: состояние + демо-рынок (несколько пар, история, симулятор) + слушает :8080.

#[tokio::main]
async fn main() {
    let state = gateway::build_state();
    gateway::seed_demo(&state).await; // BTC-USDT + alice/bob
    gateway::seed_market(&state).await; // остальные пары, история, запуск симулятора

    let app = gateway::router(state);
    let addr = "0.0.0.0:8080";
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    println!("[gateway] слушает http://{addr}  (демо-рынок: 5 пар, симулятор запущен)");
    axum::serve(listener, app).await.expect("serve");
}
