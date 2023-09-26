use std::{convert::Infallible, net::SocketAddr};

use axum::{
    extract::{Json, Path, Query},
    http::HeaderValue,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use beam::create_beam_task;
use beam_lib::{AppId, BeamClient, MsgId};
use clap::Parser;
use once_cell::sync::Lazy;
use reqwest::{header, Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use tower_http::cors::{CorsLayer, Any};
use tracing::{info, warn, Level};
use tracing_subscriber::{EnvFilter, util::SubscriberInitExt};

mod banner;
mod beam;

#[derive(Parser, Clone, Debug)]
#[clap(author, version, about, long_about = None)]
struct Config {
    /// URL of the Beam Proxy
    #[clap(env)]
    beam_url: Url,

    /// Beam AppId of this application
    #[clap(env, value_parser = |v: &str| Ok::<_, Infallible>(AppId::new_unchecked(v)))]
    beam_app: AppId,

    /// Credentials to use on the Beam Proxy
    #[clap(env)]
    beam_secret: String,

    /// The socket address this server will bind to
    #[clap(env, default_value = "0.0.0.0:8080")]
    bind_addr: SocketAddr,
}

static CONFIG: Lazy<Config> = Lazy::new(|| Config::parse());

static BEAM_CLIENT: Lazy<BeamClient> = Lazy::new(|| {
    BeamClient::new(
        &CONFIG.beam_app,
        &CONFIG.beam_secret,
        CONFIG.beam_url.clone(),
    )
});

#[tokio::main]
async fn main() {
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(Level::DEBUG)
        .with_env_filter(EnvFilter::from_default_env())
        .finish()
        .init();
    banner::print_banner();
    info!(?CONFIG);
    // TODO: Add check for reachability of beam-proxy

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_origin(Any)
        .allow_headers([header::CONTENT_TYPE]);

    let app = Router::new()
        .route("/beam", post(handle_create_beam_task))
        .route("/beam/:task_id", get(handle_listen_to_beam_tasks))
        .layer(axum::middleware::map_response(banner::set_server_header))
        .layer(cors);

    axum::Server::bind(&CONFIG.bind_addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

#[derive(Serialize, Deserialize)]
struct LensQuery {
    id: MsgId,
    sites: Vec<String>,
    query: String,
}

async fn handle_create_beam_task(
    Json(query): Json<LensQuery>,
) -> Result<impl IntoResponse, (StatusCode, &'static str)> {
    // This should just be inlined not doing it rn because of better git diff
    let LensQuery { id, sites, query } = query;
    let task = create_beam_task(id, sites, query);
    BEAM_CLIENT.post_task(&task).await.map_err(|e| {
        warn!("Unable to query Beam.Proxy: {}", e);
        (StatusCode::BAD_GATEWAY, "Unable to query Beam.Proxy")
    })?;
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
struct ListenQueryParameters {
    wait_count: u16,
}

async fn handle_listen_to_beam_tasks(
    Path(task_id): Path<MsgId>,
    Query(listen_query_parameter): Query<ListenQueryParameters>,
) -> Result<Response, (StatusCode, String)> {
    let resp = BEAM_CLIENT
        .raw_beam_request(
            Method::GET,
            &format!(
                "v1/tasks/{}/results?wait_count={}",
                task_id, listen_query_parameter.wait_count
            ),
        )
        .header(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        )
        .send()
        .await
        .map_err(|err| {
            println!("Failed request to {} with error: {}", CONFIG.beam_url, err);
            (
                StatusCode::BAD_GATEWAY,
                format!("Error calling beam, check the server logs."),
            )
        })?;
    let code = resp.status();
    if !code.is_success() {
        return Err((code, resp.text().await.unwrap_or_else(|e| e.to_string())));
    }
    Ok(convert_response(resp))
}

// Modified version of https://github.com/tokio-rs/axum/blob/c8cf147657093bff3aad5cbf2dafa336235a37c6/examples/reqwest-response/src/main.rs#L61
fn convert_response(response: reqwest::Response) -> axum::response::Response {
    let mut response_builder = Response::builder().status(response.status());

    // This unwrap is fine because we haven't insert any headers yet so there can't be any invalid
    // headers
    *response_builder.headers_mut().unwrap() = response.headers().clone();

    response_builder
        .body(axum::body::Body::wrap_stream(response.bytes_stream()))
        // Same goes for this unwrap
        .unwrap()
        .into_response()
}
