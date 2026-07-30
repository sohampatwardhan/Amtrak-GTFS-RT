use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::path::PathBuf;

#[derive(Clone)]
struct ServeState {
    dir: PathBuf,
}

pub fn router(dir: PathBuf) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/trip-updates.pb", get(trip_updates))
        .route("/vehicle-positions.pb", get(vehicle_positions))
        .route("/alerts.pb", get(alerts))
        .route("/static.zip", get(static_zip))
        .with_state(ServeState { dir })
}

async fn health() -> &'static str {
    "ok"
}

async fn serve_file(dir: &std::path::Path, name: &str, content_type: &'static str) -> Response {
    match tokio::fs::read(dir.join(name)).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type)], bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn trip_updates(State(s): State<ServeState>) -> Response {
    serve_file(&s.dir, "trip-updates.pb", "application/protobuf").await
}
async fn vehicle_positions(State(s): State<ServeState>) -> Response {
    serve_file(&s.dir, "vehicle-positions.pb", "application/protobuf").await
}
async fn alerts(State(s): State<ServeState>) -> Response {
    serve_file(&s.dir, "alerts.pb", "application/protobuf").await
}
async fn static_zip(State(s): State<ServeState>) -> Response {
    serve_file(&s.dir, "static.zip", "application/zip").await
}

pub async fn run_server(config: crate::config::Config) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "serving feeds");
    axum::serve(listener, router(config.output_dir)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_pb_and_zip_with_content_types_and_404() {
        let dir = std::env::temp_dir().join(format!("amtrak-serve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("trip-updates.pb"), b"\x08\x01").unwrap();
        std::fs::write(dir.join("static.zip"), b"PK\x03\x04").unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router(dir)).await.unwrap() });

        let client = reqwest::Client::new();

        let health = client.get(format!("http://{addr}/health")).send().await.unwrap();
        assert_eq!(health.status(), 200);

        let tu = client.get(format!("http://{addr}/trip-updates.pb")).send().await.unwrap();
        assert_eq!(tu.status(), 200);
        assert_eq!(tu.headers()[header::CONTENT_TYPE], "application/protobuf");
        assert_eq!(tu.bytes().await.unwrap().as_ref(), b"\x08\x01");

        let zip = client.get(format!("http://{addr}/static.zip")).send().await.unwrap();
        assert_eq!(zip.headers()[header::CONTENT_TYPE], "application/zip");

        let missing = client.get(format!("http://{addr}/alerts.pb")).send().await.unwrap();
        assert_eq!(missing.status(), 404);
    }
}
