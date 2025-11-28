use backend::{app, AppState};
use std::env;

#[tokio::main]
async fn main() {
    let state = if let Ok(path) = env::var("PERSIST_PATH") {
        AppState::with_persistence(path).await
    } else {
        AppState::default()
    };
    let app = app(state);
    println!("Starting server on 0.0.0.0:3000");
    axum::serve(
        tokio::net::TcpListener::bind("0.0.0.0:3000")
            .await
            .expect("bind"),
        app,
    )
    .await
    .expect("server error");
}
