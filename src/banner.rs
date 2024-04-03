use axum::{http::HeaderValue, response::Response};
use reqwest::header;
use tracing::info;

pub(crate) fn print_banner() {
    let commit = match env!("GIT_DIRTY") {
        "false" => {
            env!("GIT_COMMIT_SHORT")
        }
        _ => "SNAPSHOT",
    };
    info!(
        "🌈 Samply.Spot v{} (built {} {}, {}) ready to take requests.",
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_DATE"),
        env!("BUILD_TIME"),
        commit
    );
}

pub(crate) async fn set_server_header<B>(mut response: Response<B>) -> Response<B> {
    if !response.headers_mut().contains_key(header::SERVER) {
        response.headers_mut().insert(
            header::SERVER,
            HeaderValue::from_static(env!("SAMPLY_USER_AGENT")),
        );
    }
    response
}
