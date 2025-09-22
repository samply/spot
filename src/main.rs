use axum::{
    extract::{Json, Path, Query, State},
    http::{HeaderMap, HeaderValue},
    response::{sse::Event, IntoResponse, Sse},
    routing::{get, post},
    Router,
};
use base64::{prelude::BASE64_STANDARD, Engine};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use beam::create_beam_task;
use beam_lib::{BeamClient, MsgId, TaskResult};
use clap::Parser;
use config::Config;
use futures_util::{TryFutureExt, TryStreamExt};
use health::{BeamStatus, HealthOutput, Verdict};
use once_cell::sync::{Lazy, OnceCell};
use reqwest::{header, Method, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncWriteExt,
    net::TcpListener,
    sync::{mpsc, Mutex},
};
use tower_http::cors::CorsLayer;
use tracing::{info, warn};
use tracing_subscriber::util::SubscriberInitExt;

mod banner;
mod beam;
mod config;
mod health;

static CONFIG: Lazy<Config> = Lazy::new(Config::parse);

static LENS_QUERY_HEADER: OnceCell<String> = OnceCell::new();

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
    result_log_sender_map: ResultLogSenderMap,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or("info,hyper=warn".to_string()))
        .finish()
        .init();

    info!("{:#?}", Lazy::force(&CONFIG));

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_origin(CONFIG.cors_origin.clone())
        .allow_headers([header::CONTENT_TYPE])
        .allow_credentials(true);

    let app = Router::new()
        .route("/health", get(handler_health))
        .route("/beam", post(handle_create_beam_task))
        .route("/beam/{task_id}", get(handle_listen_to_beam_tasks))
        .route("/prism/criteria", post(handle_prism_criteria));

    let state = SharedState::default();
    let app = app
        .with_state(state)
        .layer(axum::middleware::map_response(banner::set_server_header))
        .layer(cors);

    // TODO: Add check for reachability of beam-proxy

    banner::print_banner();

    axum::serve(
        TcpListener::bind(CONFIG.bind_addr).await.unwrap(),
        app.into_make_service(),
    )
    .with_graceful_shutdown(wait_for_shutdown())
    .await
    .unwrap();
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        // Required for proper shutdown in Docker
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
        sigterm.recv().await.expect("Failed to receive SIGTERM");
        info!("Received SIGTERM, shutting down...");
        return;
    }
    // On other platforms we let the OS handle the shutdown
    #[cfg(not(unix))]
    std::future::pending::<()>().await;
}

fn default_sites() -> Vec<String> {
    CONFIG.sites.clone().unwrap_or_default()
}

fn default_wait_count() -> u16 {
    default_sites().len() as u16
}

#[derive(Serialize, Deserialize, Clone)]
struct LensQuery {
    id: MsgId,
    #[serde(default = "default_sites")]
    sites: Vec<String>,
    query: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct PrismRequest {
    #[serde(default = "default_sites")]
    sites: Vec<String>,
}

async fn handler_health() -> Json<HealthOutput> {
    Json(HealthOutput {
        summary: Verdict::Healthy,
        beam: BeamStatus::Ok,
    })
}

fn verify_query(query: &LensQuery) -> Result<(), (StatusCode, &'static str)> {
    let decoded = BASE64_STANDARD
        .decode(&query.query)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Query is not valid base64"))?;
    let json = serde_json::from_slice::<serde_json::Value>(&decoded)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Query is not valid JSON"))?;
    if json["lang"] != "cql" {
        return Ok(());
    }
    let cql_enc = json["lib"]["content"][0]["data"].as_str().ok_or((
        StatusCode::BAD_REQUEST,
        "Query does not contain a CQL payload",
    ))?;
    let cql = BASE64_STANDARD
        .decode(cql_enc)
        .map_err(|_| (StatusCode::BAD_REQUEST, "CQL payload is not valid base64"))?;
    let cql = String::from_utf8(cql)
        .map_err(|_| (StatusCode::BAD_REQUEST, "CQL payload is not valid UTF-8"))?;
    let Some(project) = &CONFIG.project else {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Project is not configured for CQL validation",
        ));
    };
    let query_header = LENS_QUERY_HEADER.get_or_try_init(|| {
        Ok(
            std::fs::read_to_string(format!("/lens_queries/{project}.cql"))
                .map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to read query header file",
                    )
                })?
                .trim()
                .to_string(),
        )
    })?;
    let Some(user_defined_query) = cql.strip_prefix(query_header.as_str()) else {
        warn!(%query_header, %cql, "Header missmatch");
        return Err((
            StatusCode::BAD_REQUEST,
            "CQL query does not start with the required header",
        ));
    };
    let placeholders: Vec<&str> = vec![
        "BBMRI_STRAT_AGE_STRATIFIER",
        "BBMRI_STRAT_CUSTODIAN_STRATIFIER",
        "BBMRI_STRAT_DEF_IN_INITIAL_POPULATION",
        "BBMRI_STRAT_DEF_SPECIMEN",
        "BBMRI_STRAT_DIAGNOSIS_STRATIFIER",
        "BBMRI_STRAT_GENDER_STRATIFIER",
        "BBMRI_STRAT_SAMPLE_TYPE_STRATIFIER",
        "BBMRI_STRAT_STORAGE_TEMPERATURE_STRATIFIER",
        "DHKI_STRAT_AGE_STRATIFIER",
        "DHKI_STRAT_ENCOUNTER_STRATIFIER",
        "DHKI_STRAT_MEDICATION_STRATIFIER",
        "DHKI_STRAT_SPECIMEN_STRATIFIER",
        "DKTK_REPLACE_HISTOLOGY_STRATIFIER",
        "DKTK_REPLACE_SPECIMEN_STRATIFIER",
        "DKTK_STRAT_AGE_CLASS_STRATIFIER",
        "DKTK_STRAT_AGE_STRATIFIER",
        "DKTK_STRAT_DECEASED_STRATIFIER",
        "DKTK_STRAT_DEF_IN_INITIAL_POPULATION",
        "DKTK_STRAT_DIAGNOSIS_STRATIFIER",
        "DKTK_STRAT_ENCOUNTER_STRATIFIER",
        "DKTK_STRAT_GENDER_STRATIFIER",
        "DKTK_STRAT_GENETIC_VARIANT",
        "DKTK_STRAT_HISTOLOGY_STRATIFIER",
        "DKTK_STRAT_MEDICATION_STRATIFIER",
        "DKTK_STRAT_PRIMARY_DIAGNOSIS_NO_SORT_STRATIFIER",
        "DKTK_STRAT_PRIMARY_DIAGNOSIS_STRATIFIER",
        "DKTK_STRAT_PROCEDURE_STRATIFIER",
        "DKTK_STRAT_SPECIMEN_STRATIFIER",
        "EHDS2_IN_INITIAL_POPULATION",
        "EHDS2_OBSERVATION",
        "EHDS2_PATIENT",
        "EHDS2_SPECIMEN",
        "EHDS2_UTIL",
        "EXLIQUID_CQL_AVAILABLE_SPECIMEN",
        "EXLIQUID_CQL_DIAGNOSIS",
        "EXLIQUID_CQL_SPECIMEN",
        "EXLIQUID_STRAT_DEF_IN_INITIAL_POPULATION",
        "ITCC_STRAT_AGE_CLASS_STRATIFIER",
        "ITCC_STRAT_DIAGNOSIS_STRATIFIER",
        "MTBA_STRAT_GENETIC_VARIANT",
        "PRISM_STRAT_AGE_STRATIFIER_BBMRI",
        "UCT_STRAT_SPECIMEN_STRATIFIER",
    ];
    if placeholders.iter().any(|ph| user_defined_query.contains(ph)) {
        return Err((
            StatusCode::BAD_REQUEST,
            "CQL query contains forbidden placeholder",
        ));
    }
    Ok(())
}

fn check_lang(query: &LensQuery) -> Result<(), (StatusCode, &'static str)> {
    if let Some(allowed_lang) = &CONFIG.allowed_lang {
        let decoded = BASE64_STANDARD
            .decode(&query.query)
            .map_err(|_| (StatusCode::BAD_REQUEST, "Query is not valid base64"))?;
        let json = serde_json::from_slice::<serde_json::Value>(&decoded)
            .map_err(|_| (StatusCode::BAD_REQUEST, "Query is not valid JSON"))?;
        if json["lang"].as_str() != Some(allowed_lang) {
            return Err((StatusCode::BAD_REQUEST, "Query is not allowed"));
        }
    }
    Ok(())
}

async fn handle_create_beam_task(
    State(SharedState {
        result_log_sender_map,
        ..
    }): State<SharedState>,
    headers: HeaderMap,
    Json(query): Json<LensQuery>,
) -> Result<impl IntoResponse, (StatusCode, &'static str)> {
    if query.sites.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No sites specified"));
    }
    check_lang(&query)?;
    verify_query(&query)?;

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
            BEAM_CLIENT
                .post_task(&task)
                .await
                .map_err(|e| {
                    warn!("Unable to query Beam.Proxy: {}", e);
                    (StatusCode::BAD_GATEWAY, "Unable to query Beam.Proxy")
                })
                .map(|()| StatusCode::CREATED)
        }
        Err(e) => {
            warn!("Unable to query Beam.Proxy: {}", e);
            Err((StatusCode::BAD_GATEWAY, "Unable to query Beam.Proxy"))
        }
    }
}

#[derive(Deserialize)]
struct ListenQueryParameters {
    #[serde(default = "default_wait_count")]
    wait_count: u16,
}

async fn handle_listen_to_beam_tasks(
    Path(task_id): Path<MsgId>,
    Query(listen_query_parameter): Query<ListenQueryParameters>,
    State(SharedState {
        result_log_sender_map,
        ..
    }): State<SharedState>,
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
    let sender = result_log_sender_map.lock().await.remove(&task_id);
    if sender.is_none() && CONFIG.log_file.is_some() {
        warn!("Logging is enabled but no log sender found for logging results.");
    }
    let stream = async_sse::decode(
        resp.bytes_stream()
            .map_err(io::Error::other)
            .into_async_read(),
    )
    .and_then(move |event| {
        let sender = sender.clone();
        async move {
            match event {
                async_sse::Event::Retry(_) => unreachable!("Beam does not send retries!"),
                async_sse::Event::Message(m) => {
                    if let Ok(result) =
                        serde_json::from_slice::<TaskResult<beam_lib::RawString>>(m.data())
                    {
                        if result.status == beam_lib::WorkStatus::Succeeded {
                            if let Some(sender) = sender {
                                sender
                                    .send(
                                        result.from.as_ref().split('.').nth(1).unwrap().to_owned(),
                                    )
                                    .await
                                    .expect("not dropped");
                            }
                        }
                    }
                    Ok(Event::default()
                        .data(String::from_utf8_lossy(m.data()))
                        .event(m.name()))
                }
            }
        }
    });
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
        body: PrismRequest,
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
        ts: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
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
    Json(body): Json<PrismRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let Some(base) = CONFIG.prism_url.clone() else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Prism URL is not configured".to_string(),
        ));
    };
    static CLIENT: std::sync::LazyLock<reqwest::Client> =
        std::sync::LazyLock::new(reqwest::Client::new);
    let url = format!("{}/criteria", base.to_string().trim_end_matches('/'));
    let resp = CLIENT.post(&url).json(&body).send().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Failed to reach prism: {e}"),
        )
    })?;
    // Logging
    if let Some(log_file) = &CONFIG.log_file {
        log_endpoint(&log_file, &headers, LogEvent::Prism { body }).await;
    }
    Ok(axum::response::Response::from(resp))
}
