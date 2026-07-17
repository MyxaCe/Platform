//! Бинарь шлюза: собирает состояние, наполняет демо-данными и слушает :8080.

#[tokio::main]
async fn main() {
    let state = gateway::build_state();
    gateway::seed_demo(&state).await;

    let app = gateway::router(state);
    let addr = "0.0.0.0:8080";
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    println!("[gateway] слушает http://{addr}  (demo: BTC-USDT, токены alice-token/bob-token)");
    axum::serve(listener, app).await.expect("serve");
}
