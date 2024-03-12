use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Json, Path, Query, State},
    http::HeaderValue,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use beam::create_beam_task;
use beam_lib::{BeamClient, MsgId};
use clap::Parser;
use config::Config;
use once_cell::sync::Lazy;
use reqwest::{header, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tracing::{info, warn, Level};
use tracing_subscriber::{util::SubscriberInitExt, EnvFilter};

mod banner;
mod beam;
mod catalogue;
mod config;

static CONFIG: Lazy<Config> = Lazy::new(Config::parse);

static BEAM_CLIENT: Lazy<BeamClient> = Lazy::new(|| {
    BeamClient::new(
        &CONFIG.beam_app_id,
        &CONFIG.beam_secret,
        CONFIG.beam_proxy_url.clone(),
    )
});

#[derive(Clone)]
struct SharedState {
    extended_json: Arc<Mutex<Value>>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(Level::DEBUG)
        .with_env_filter(EnvFilter::from_default_env())
        .finish()
        .init();

    // TODO: Remove this workaround once clap manages to not choke on URL "".
    if let Ok(var) = std::env::var("CATALOGUE_URL") {
        if var.is_empty() {
            std::env::remove_var("CATALOGUE_URL");
        }
    }

    info!("{:#?}", Lazy::force(&CONFIG));

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_origin(CONFIG.cors_origin.clone())
        .allow_headers([header::CONTENT_TYPE]);

    let make_service = if let Some(url) = CONFIG.catalogue_url.clone() {
        let extended_json = catalogue::spawn_thing(url, CONFIG.prism_url.clone());
        let state = SharedState { extended_json };

        let app = Router::new()
            .route("/beam", post(handle_create_beam_task))
            .route("/beam/:task_id", get(handle_listen_to_beam_tasks))
            .route("/catalogue", get(handle_get_catalogue))
            .with_state(state)
            .layer(axum::middleware::map_response(banner::set_server_header))
            .layer(cors);

        app.into_make_service()
    } else {
        let app = Router::new()
            .route("/beam", post(handle_create_beam_task))
            .route("/beam/:task_id", get(handle_listen_to_beam_tasks))
            .layer(axum::middleware::map_response(banner::set_server_header))
            .layer(cors);

        app.into_make_service()
    };

    // TODO: Add check for reachability of beam-proxy

    banner::print_banner();

    axum::Server::bind(&CONFIG.bind_addr)
        .serve(make_service)
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
    if let Some(log_file) = &CONFIG.log_file {
        #[derive(Serialize)]
        struct Log<'a> {
            #[serde(flatten)]
            query: &'a LensQuery,
            ts: u64
        }
        let json = serde_json::to_vec(&Log {
            query: &query,
            ts: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
        }).expect("Failed to serialize log");
        tokio::spawn(async move {
            if let Err(e) = tokio::fs::write(log_file, json).await {
                warn!("Failed to write to log file: {e}");
            };
        });
    }
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
            println!(
                "Failed request to {} with error: {}",
                CONFIG.beam_proxy_url, err
            );
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

async fn handle_get_catalogue(State(state): State<SharedState>) -> Json<Value> {
    // TODO: We can totally avoid this clone by using axum_extra ErasedJson
    Json(state.extended_json.lock().await.clone())
}
