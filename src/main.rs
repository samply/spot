use std::{sync::Arc, convert::Infallible};

use axum::{Router, routing::{get, post}, extract::{Json, State, Path, Query}, response::{Sse, sse::Event, IntoResponse, Response}, http::{HeaderValue, HeaderName}};
use clap::Parser;
use futures::{Stream, TryStreamExt, StreamExt};
use hyper::{Client, HeaderMap, header::{AUTHORIZATION, ACCEPT}, Uri, Request, Method, Body};
use reqwest::{Url, StatusCode};
use serde::{Serialize, Deserialize};
use tracing::{debug, trace};
use uuid::Uuid;

use crate::logger::init_logger;

mod banner;
mod logger;

#[derive(Parser, Clone)]
#[clap(author, version, about, long_about = None)]
struct Arguments {
    /// URL of the Beam Proxy
    beam_url: Url,
    /// Credentials to use on the Beam Proxy
    beam_app: String,
    /// Credentials to use on the Beam Proxy
    beam_secret: String,
}

#[derive(Serialize, Deserialize)]
struct LensQuery {
    id: String,
    sites: Vec<String>,
    query: String
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailureStrategy {
    Retry(Retry),
}

#[derive(Serialize, Deserialize)]
pub struct Retry {
    pub backoff_millisecs: usize,
    pub max_tries: usize,
}

// Taken from beam, can this be a library?
pub enum SseEventType {
    NewTask,
    NewResult,
    UpdatedTask,
    UpdatedResult,
    WaitExpired,
    DeletedTask,
    Error,
    Undefined,
    Unknown(String)
}

// Taken from beam, can this be a library?
impl AsRef<str> for SseEventType {
    fn as_ref(&self) -> &str {
        match self {
            SseEventType::NewTask => "new_task",
            SseEventType::NewResult => "new_result",
            SseEventType::UpdatedTask => "updated_task",
            SseEventType::UpdatedResult => "updated_result",
            SseEventType::WaitExpired => "wait_expired",
            SseEventType::DeletedTask => "deleted_task",
            SseEventType::Error => "error",
            SseEventType::Undefined => "", // Make this "message"?
            SseEventType::Unknown(e) => e.as_str(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct BeamTask {
    id: String,
    from: String,
    to: Vec<String>,
    metadata: String,
    body: String,
    failure_strategy: FailureStrategy,
    ttl: String,
}

async fn handle_create_beam_task(
    State(args): State<Arc<Arguments>>,
    Json(query): Json<LensQuery>
) -> Result<impl IntoResponse, (StatusCode, &'static str)>{
    let result = create_beam_task(&args, &query).await?;
    Ok(result)
}

async fn handle_listen_to_beam_tasks(
    State(args): State<Arc<Arguments>>,
    Path(task_id): Path<Uuid>,
    Query(wait_count): Query<i32>
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let result = listen_beam_results(&args, task_id.to_string(), wait_count).await?;
    Ok(result)
}

#[tokio::main]
async fn main() {
    init_logger().expect("Unable to initialize logger.");
    let args = Arc::new(Arguments::parse());
    debug!("Beam URL: {}", &args.beam_url);
    debug!("Beam App: {}", &args.beam_app);
    // TODO: Add check for reachability of beam-proxy
    let app = Router::new()
        .route("/beam", post(handle_create_beam_task))
        .route("/beam/:task_id", get(handle_listen_to_beam_tasks))
        .layer(axum::middleware::map_response(banner::set_server_header))
        .with_state(args);

    axum::Server::bind(&"0.0.0.0:8080".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn create_beam_task(args: &Arguments, query: &LensQuery) -> Result<impl IntoResponse, (StatusCode, &'static str)> {
    let client = Client::new();
    let url = format!("{}v1/tasks", args.beam_url);
    let auth_header = generate_auth_header(args);
    let body = map_lens_to_beam(args, query);
    trace!(url, auth_header, body.id);
    let body = serde_json::to_vec(&body).unwrap();
    let body = hyper::Body::from(body);
    let req = Request::builder()
        .method(Method::POST)
        .uri(url)
        .header(AUTHORIZATION, auth_header)
        .body(body)
        .map_err(|e| {
            println!("Unable to construct Beam.Proxy query: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Unable to build Beam.Proxy query.")
        })?;
    let resp = client
        .request(req)
        .await
        .map_err(|e| {
            println!("Unable to query Beam.Proxy: {}", e);
            (StatusCode::BAD_GATEWAY, "Unable to query Beam.Proxy.")
        })?;
    Ok(resp)
}

async fn listen_beam_results(args: &Arguments, task_id: String, wait_count: i32) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let client = reqwest::Client::new();
    let url = format!("{}v1/tasks/{}/results?wait_count={}", args.beam_url, task_id, wait_count);
    let mut headers = HeaderMap::new();
    println!("{}", url);
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("ApiKey {}.dev-torben.broker.dev.ccp-it.dktk.dkfz.de {}", args.beam_app, args.beam_secret))
            .map_err(|_err| {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Can't assemble authorization Header for Beam"))
            })?);
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));

    let resp = client.get(url)
          .headers(headers)
          .send()
          .await.map_err(|err| {
              println!("Failed request to {} with error: {}", args.beam_url, err.to_string());
              (StatusCode::BAD_GATEWAY, format!("Error calling beam, check the server logs."))
          })?;

    let code = resp.status();

    if ! code.is_success() {
        // How can i get the message from resp?
        return Err((code, format!("TODO: Proper handling of the error message here")))
    }

    let outgoing = async_stream::stream! {

        let incoming = resp
            .bytes_stream()
            .map_err(|e| futures::io::Error::new(futures::io::ErrorKind::Other, e));

        let mut decoder = async_sse::decode(incoming.into_async_read());

        while let Some(event) = decoder.next().await {
            let event = match event {
                Ok(event) => event,
                Err(err) => {
                    println!("Got error reading SSE stream: {err}");
                    yield Ok(Event::default()
                             .event(SseEventType::Error)
                             .data("Error reading SSE stream from Broker (see Proxy logs for details)."));
                    continue;
                }
            };
            match event {
                async_sse::Event::Retry(_dur) => {
                    format!("Got a retry message from the Broker, which is not yet supported.");
                },
                async_sse::Event::Message(_event) => {
                    continue;
                }
            }
        }
    };

    let sse = Sse::new(outgoing);
    Ok(sse)
}

fn generate_auth_header (args: &Arguments) -> String {
    format!("ApiKey {}.dev-torben.broker.dev.ccp-it.dktk.dkfz.de {}", args.beam_app, args.beam_secret)
}


fn map_lens_to_beam(args: &Arguments, query: &LensQuery) -> BeamTask {
    let task_id = Uuid::new_v4();
    let mut target_sites = Vec::new();
    for site in query.sites.clone() {
        target_sites.push(format!("focus.{}.broker.dev.ccp-it.dktk.dkfz.de", site));
    }
    BeamTask {
        id: format!("{}", task_id),
        from: format!("{}.dev-torben.broker.dev.ccp-it.dktk.dkfz.de", args.beam_app),
        to: target_sites,
        metadata: format!(""),
        body: query.query.clone(),
        failure_strategy: FailureStrategy::Retry(Retry {
            backoff_millisecs: 1000,
            max_tries: 5
        }),
        ttl: format!("360s")
    }
}
