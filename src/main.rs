use axum::{
    extract::{Json, Path, Query, State},
    http::HeaderValue,
    response::{sse::Event, IntoResponse, Sse},
    routing::{get, post},
    Router
};
use std::{collections::HashMap, io, path::PathBuf, sync::Arc, time::{SystemTime, UNIX_EPOCH}};

use beam::create_beam_task;
use beam_lib::{BeamClient, MsgId, TaskResult};
use clap::Parser;
use config::Config;
use once_cell::sync::Lazy;
use reqwest::{header, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_http::cors::CorsLayer;
use tracing::{info, warn, Level};
use tracing_subscriber::{util::SubscriberInitExt, EnvFilter};
use tokio::{io::AsyncWriteExt, sync::{oneshot, Mutex}};
use futures_util::{TryFutureExt, TryStreamExt};

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

type ResultLogSenderMap = Arc<Mutex<HashMap<MsgId, oneshot::Sender<u32>>>>;

#[derive(Clone, Default)]
struct SharedState {
    extended_json: Arc<Mutex<Value>>,
    result_log_sender_map: ResultLogSenderMap,
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
    
    let mut app = Router::new()
        .route("/beam", post(handle_create_beam_task))
        .route("/beam/:task_id", get(handle_listen_to_beam_tasks));

    let state = if let Some(url) = CONFIG.catalogue_url.clone() {
        let extended_json = catalogue::spawn_thing(url, CONFIG.prism_url.clone());
        app = app.route("/catalogue", get(handle_get_catalogue));
        SharedState { extended_json, result_log_sender_map: Default::default() }
    } else {
        SharedState::default()
    };
    let app = app.with_state(state)
        .layer(axum::middleware::map_response(banner::set_server_header))
        .layer(cors);

    // TODO: Add check for reachability of beam-proxy

    banner::print_banner();

    axum::Server::bind(&CONFIG.bind_addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

#[derive(Serialize, Deserialize, Clone)]
struct LensQuery {
    id: MsgId,
    sites: Vec<String>,
    query: String,
}

async fn handle_create_beam_task(
    State(SharedState { result_log_sender_map, .. }): State<SharedState>,
    Json(query): Json<LensQuery>,
) -> Result<impl IntoResponse, (StatusCode, &'static str)> {
    if let Some(log_file) = &CONFIG.log_file {
        tokio::spawn(log_query(log_file, query.clone(), result_log_sender_map));
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
    State(SharedState { result_log_sender_map, .. }): State<SharedState>
) -> Result<impl IntoResponse, (StatusCode, String)> {
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
    let counter = Counter {
        value: Default::default(),
        sender: result_log_sender_map.lock().await.remove(&task_id),
    };
    let stream = async_sse::decode(resp.bytes_stream().map_err(|e| io::Error::new(io::ErrorKind::Other, e)).into_async_read())
        .map_ok(move |event| match event {
            async_sse::Event::Retry(_) => unreachable!("Beam does not send retries!"),
            async_sse::Event::Message(m) => {
                if serde_json::from_slice::<TaskResult<beam_lib::RawString>>(m.data()).is_ok_and(|v| v.status == beam_lib::WorkStatus::Succeeded) {
                    counter.value.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Event::default().data(String::from_utf8_lossy(m.data())).event(m.name())
            },
        });
    Ok(Sse::new(stream))
}

async fn log_query(log_file: &PathBuf, query: LensQuery, result_logger_map: ResultLogSenderMap) {
    #[derive(Serialize)]
    struct Log {
        #[serde(flatten)]
        query: LensQuery,
        ts: u128,
        results: u32
    }
    let (tx, rx) = oneshot::channel();
    result_logger_map.lock().await.insert(query.id, tx);
    let results = rx.await.expect("Sender is never dropped");
    let mut out = serde_json::to_vec(&Log {
        query,
        results,
        ts: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()
    }).expect("Failed to serialize log");
    out.push('\n' as u8);
    let res = tokio::fs::OpenOptions::new()
        .append(true)
        .open(log_file)
        .and_then(|mut f| async move { f.write(&out).await })
        .await;
    if let Err(e) = res {
        warn!("Failed to write to log file: {e}");
    };
}

struct Counter {
    value: std::sync::Arc<std::sync::atomic::AtomicU32>,
    sender: Option<oneshot::Sender<u32>>,
}

impl Drop for Counter {
    fn drop(&mut self) {
        let received = self.value.load(std::sync::atomic::Ordering::Relaxed);
        info!("Received {} results.", received);
        if let Some(s) = self.sender.take() {
            _ = s.send(received);
        }
    }
}

async fn handle_get_catalogue(State(state): State<SharedState>) -> Json<Value> {
    // TODO: We can totally avoid this clone by using axum_extra ErasedJson
    Json(state.extended_json.lock().await.clone())
}
