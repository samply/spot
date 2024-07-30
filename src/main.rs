use axum::{
    body::Bytes, extract::{Json, Path, Query, State}, http::{HeaderMap, HeaderValue}, response::{sse::Event, IntoResponse, Sse}, routing::{get, post}, Router
};
use mini_moka::sync::Cache;
use std::{convert::Infallible, sync::Arc, time::Duration};

use beam_lib::{AppId, MsgId, RawString, TaskResult};
use clap::Parser;
use config::Config;
use once_cell::sync::Lazy;
use reqwest::{header, Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_http::cors::CorsLayer;
use tracing::{info, warn, Level};
use tracing_subscriber::{util::SubscriberInitExt, EnvFilter};
use tokio::{net::TcpListener, time::Instant};
use health::{BeamStatus, HealthOutput, Verdict};

mod banner;
mod config;
mod health;

static CONFIG: Lazy<Config> = Lazy::new(Config::parse);

static BLAZE_CLIENT: Lazy<Client> = Lazy::new(|| Client::builder().default_headers(HeaderMap::from_iter([(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))])).build().unwrap());

#[derive(Clone)]
struct SharedState {
    results: Cache<MsgId, Arc<TaskResult<RawString>>>
}

impl SharedState {
    fn new() -> Self {
        Self { results: Cache::builder().time_to_live(Duration::from_secs(2 * 60)).build() }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(Level::DEBUG)
        .with_env_filter(EnvFilter::from_default_env())
        .finish()
        .init();

    info!("{:#?}", Lazy::force(&CONFIG));

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_origin(CONFIG.cors_origin.clone())
        .allow_headers([header::CONTENT_TYPE]);
    
    let mut app = Router::new()
        .route("/health", get(handler_health))
        .route("/beam", post(handle_create_beam_task))
        .route("/beam/:task_id", get(handle_listen_to_beam_tasks));

    let app = app
        .layer(axum::middleware::map_response(banner::set_server_header))
        .layer(cors)
        .with_state(SharedState::new());

    banner::print_banner();

    axum::serve(TcpListener::bind(&CONFIG.bind_addr).await.unwrap(), app)
        .await
        .unwrap();
}

#[derive(Serialize, Deserialize, Clone)]
struct LensQuery {
    id: MsgId,
    sites: Vec<String>,
    query: String,
}

async fn handler_health() -> Json<HealthOutput> {
    Json(HealthOutput {
        summary: Verdict::Healthy,
        beam: BeamStatus::Ok
    })
}

async fn handle_create_beam_task(
    State(SharedState { results }): State<SharedState>,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    let query = serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    #[derive(Serialize, Deserialize, Debug)]
    pub struct CqlQuery {
        pub lib: Value,
        pub measure: Value
    }
    let LensQuery { id, query, .. } = query;
    let query: CqlQuery = serde_json::from_str(&query).map_err(|_| StatusCode::BAD_REQUEST)?;
    let measure_url = query.measure["url"].as_str().ok_or_else(|| {
        warn!("Measure did not contain valid url: {:#?}", query.measure);
        StatusCode::BAD_REQUEST
    })?;
    BLAZE_CLIENT
        .post(format!("{}Library", CONFIG.blaze_url))
        .body(query.lib.to_string())
        .send()
        .await
        .map_err(|e| {
            warn!("Blaze req failed: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .error_for_status()
        .map_err(|e| {
            warn!("Blaze unexpected status: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    BLAZE_CLIENT
        .post(format!("{}Measure", CONFIG.blaze_url))
        .header("Content-Type", "application/json")
        .body(query.measure.to_string())
        .send()
        .await
        .map_err(|e| {
            warn!("Blaze req failed: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .error_for_status()
        .map_err(|e| {
            warn!("Blaze unexpected status: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let result = BLAZE_CLIENT.get(format!("{}Measure/$evaluate-measure?measure={}&periodStart=2000&periodEnd=2030", CONFIG.blaze_url, measure_url))
        .send()
        .await
        .map_err(|e| {
            warn!("Blaze req failed: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .error_for_status()
        .map_err(|e| {
            warn!("Blaze unexpected status: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .text()
        .await
        .map_err(|_| {
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    results.insert(id, Arc::new(TaskResult { from: AppId::new_unchecked("local.local.broker"), to: vec![AppId::new_unchecked("local.local.broker")], task: id, status: beam_lib::WorkStatus::Succeeded, body: RawString(result), metadata: Value::Null }));
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
struct ListenQueryParameters {
    wait_count: u16,
}

async fn handle_listen_to_beam_tasks(
    Path(task_id): Path<MsgId>,
    Query(_listen_query_parameter): Query<ListenQueryParameters>,
    State(SharedState { results }): State<SharedState>
) -> Result<impl IntoResponse, StatusCode> {
    // TODO: Send the result via a channel instead
    let in_30s = Instant::now() + Duration::from_secs(30);
    let result = loop {
        match results.get(&task_id) {
            Some(res) => break res,
            None if !(Instant::now() > in_30s) => tokio::time::sleep(Duration::from_millis(250)).await,
            None => return Err(StatusCode::NO_CONTENT)
        }
    };
    let event = Event::default().data(serde_json::to_string(result.as_ref()).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?);
    Ok(Sse::new(futures_util::stream::once(async { Ok::<_, Infallible>(event) })))
}
