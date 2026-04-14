use axum::{Router, routing::get};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "ok" }));
    let listener = TcpListener::bind("0.0.0.0:8080")
        .await
        .expect("fixture should bind to port 8080");

    axum::serve(listener, app)
        .await
        .expect("fixture server should run");
}
