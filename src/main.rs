use axum::{
    extract::{Json, Path, Query, State},
    http::{HeaderMap, HeaderValue},
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
use tokio::{io::AsyncWriteExt, net::TcpListener, sync::{mpsc, Mutex}};
use futures_util::{TryFutureExt, TryStreamExt};
use health::{BeamStatus, HealthOutput, Verdict};

mod banner;
mod beam;
mod catalogue;
mod config;
mod health;

static CONFIG: Lazy<Config> = Lazy::new(Config::parse);

static BEAM_CLIENT: Lazy<BeamClient> = Lazy::new(|| {
    BeamClient::new(
        &CONFIG.beam_app_id,
        &CONFIG.beam_secret,
        CONFIG.beam_proxy_url.clone(),
    )
});

type ResultLogSenderMap = Arc<Mutex<HashMap<MsgId, mpsc::Sender<String>>>>;

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
        .allow_headers([header::CONTENT_TYPE])
        .allow_credentials(true);
    
    let mut app = Router::new()
        .route("/health", get(handler_health))
        .route("/beam", post(handle_create_beam_task))
        .route("/beam/{task_id}", get(handle_listen_to_beam_tasks))
        .route("/prism/criteria", post(handle_prism_criteria));

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

    axum::serve(TcpListener::bind(CONFIG.bind_addr).await.unwrap(), app.into_make_service())
        .with_graceful_shutdown(wait_for_shutdown())
        .await
        .unwrap();
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        // Required for proper shutdown in Docker
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
        sigterm.recv().await.expect("Failed to receive SIGTERM");
        info!("Received SIGTERM, shutting down...");
        return
    }
    // On other platforms we let the OS handle the shutdown
    #[cfg(not(unix))]
    std::future::pending::<()>().await;
}

fn default_sites() -> Vec<String> {
    CONFIG.sites.clone().unwrap_or_default()
}

#[derive(Serialize, Deserialize, Clone)]
struct LensQuery {
    id: MsgId,
    #[serde(default="default_sites")]
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
    State(SharedState { result_log_sender_map, .. }): State<SharedState>,
    headers: HeaderMap,
    Json(query): Json<LensQuery>,
) -> Result<impl IntoResponse, (StatusCode, &'static str)> {
    if let Some(log_file) = &CONFIG.log_file {
        let q = query.clone();
        let h = headers.clone();
        let log_file = log_file.clone();
        let result_log_sender_map = result_log_sender_map.clone();
        tokio::spawn(async move {
            let (tx, mut rx) = mpsc::channel(q.sites.len());
            result_log_sender_map.lock().await.insert(q.id, tx);
            let mut results = Vec::with_capacity(q.sites.len());
            while let Some(result) = rx.recv().await {
                results.push(result);
            }
            log_endpoint(&log_file, &h, LogEvent::Query { query: q, results }).await;
        });
    }
    let LensQuery { id, sites, query } = query;

    // Check if the query is allowed
    if let Some(filter) = &CONFIG.query_filter {
        if !filter.contains(&query) {
            return Err((StatusCode::FORBIDDEN, "Query not allowed"));
        }
    }

    let mut task = create_beam_task(id, sites, query);
    match BEAM_CLIENT.post_task(&task).await {
        Ok(()) => Ok(StatusCode::CREATED),
        Err(beam_lib::BeamError::InvalidReceivers(invalid)) => {
            task.to.retain(|t| !invalid.contains(&t.proxy_id()));
            BEAM_CLIENT.post_task(&task).await
                .map_err(|e| {
                    warn!("Unable to query Beam.Proxy: {}", e);
                    (StatusCode::BAD_GATEWAY, "Unable to query Beam.Proxy")
                })
                .map(|()| StatusCode::CREATED)
        },
        Err(e) => {
            warn!("Unable to query Beam.Proxy: {}", e);
            Err((StatusCode::BAD_GATEWAY, "Unable to query Beam.Proxy"))
        }
    }
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
                "Error calling beam, check the server logs.".to_owned(),
            )
        })?;
    let code = resp.status();
    if !code.is_success() {
        return Err((code, resp.text().await.unwrap_or_else(|e| e.to_string())));
    }
    let sender =  result_log_sender_map.lock().await.remove(&task_id);
    if sender.is_none() && CONFIG.log_file.is_some() {
        warn!("Logging is enabled but no log sender found for logging results.");
    }
    let stream = async_sse::decode(resp.bytes_stream().map_err(io::Error::other).into_async_read())
        .and_then(move |event| {
            let sender = sender.clone();
            async move { match event {
                async_sse::Event::Retry(_) => unreachable!("Beam does not send retries!"),
                async_sse::Event::Message(m) => {
                    if let Ok(result) = serde_json::from_slice::<TaskResult<beam_lib::RawString>>(m.data()) {
                        if result.status == beam_lib::WorkStatus::Succeeded {
                            if let Some(sender) = sender {
                                sender.send(result.from.as_ref().split('.').nth(1).unwrap().to_owned()).await.expect("not dropped");
                            }
                        }
                    }
                    Ok(Event::default().data(String::from_utf8_lossy(m.data())).event(m.name()))
                },
            }
        }});
    Ok(Sse::new(stream))
}

#[derive(Serialize)]
struct EndpointLog {
    user_email: String,
    ts: u128,
    #[serde(flatten)]
    log_type: LogEvent,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum LogEvent {
    Query {
        query: LensQuery,
        results: Vec<String>,
    },
    Prism {
        body: String,
    },
}

async fn log_endpoint(log_file: &PathBuf, headers: &HeaderMap, log_type: LogEvent) {
    let user_email = headers
        .get("x-auth-request-email")
        .unwrap_or(&HeaderValue::from_static("Unknown user"))
        .to_str()
        .unwrap_or("Unknown user")
        .to_owned();
    let log = EndpointLog {
        user_email,
        ts: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
        log_type,
    };
    let mut out = serde_json::to_vec(&log).expect("Failed to serialize log");
    out.push(b'\n');
    tracing::debug!(target: "spot.log", "Writing to log file: {}", String::from_utf8_lossy(&out));
    let res = tokio::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(log_file)
        .and_then(|mut f| async move {
            f.write(&out).await?;
            f.flush().await
        })
        .await;
    if let Err(e) = res {
        warn!("Failed to write to log file: {e}");
    }
}

async fn handle_prism_criteria(
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(reqwest::Client::new);
    let base = CONFIG.prism_url.to_string();
    let url = format!("{}/criteria", base.trim_end_matches('/'));
    let resp = CLIENT
        .post(&url)
        .headers(headers.clone())
        .body(body.clone())
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Failed to reach prism: {e}")))?;
    // Logging
    if let Some(log_file) = &CONFIG.log_file {
        log_endpoint(&log_file, &headers, LogEvent::Prism { body: String::from_utf8_lossy(&body).to_string() }).await;
    }
    Ok(axum::response::Response::from(resp))
}

async fn handle_get_catalogue(State(state): State<SharedState>) -> Json<Value> {
    // TODO: We can totally avoid this clone by using axum_extra ErasedJson
    Json(state.extended_json.lock().await.clone())
}
