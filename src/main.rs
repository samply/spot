use std::{sync::Arc, convert::Infallible, str::FromStr, fmt::Display};

use axum::{Router, routing::{get, post}, extract::{Json, State, Path, Query}, response::{Sse, sse::Event, IntoResponse}, http::HeaderValue};
use clap::Parser;
use futures::{Stream, TryStreamExt, StreamExt};
use http::{StatusCode, header::LOCATION};
use hyper::{Client, header::{AUTHORIZATION, ACCEPT}, Uri, Request, Method, Body};
use serde::{Serialize, Deserialize};
use tracing::{debug, trace, info, warn, error};
use url::Url;
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

impl FromStr for SseEventType {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "new_task" => Self::NewTask,
            "new_result" => Self::NewResult,
            "updated_task" => Self::UpdatedTask,
            "updated_result" => Self::UpdatedResult,
            "wait_expired" => Self::WaitExpired,
            "deleted_task" => Self::DeletedTask,
            "error" => Self::Error,
            "message" => Self::Undefined,
            unknown => Self::Unknown(unknown.to_string())
        })
    }
}

impl Display for SseEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_ref())
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

#[derive(Deserialize)]
struct ListenQueryParameters {
    wait_count: u16,
}

async fn handle_listen_to_beam_tasks(
    State(args): State<Arc<Arguments>>,
    Path(task_id): Path<Uuid>,
    Query(listen_query_parameter): Query<ListenQueryParameters>
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let result = listen_beam_results(&args, task_id.to_string(), listen_query_parameter.wait_count).await?;
    Ok(result)
}

#[tokio::main]
async fn main() {
    init_logger().expect("Unable to initialize logger.");
    banner::print_banner();
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
    let beam_resp = client
        .request(req)
        .await
        .map_err(|e| {
            println!("Unable to query Beam.Proxy: {}", e);
            (StatusCode::BAD_GATEWAY, "Unable to query Beam.Proxy.")
        })?;
    // TODO: Handle 401 etc. here

    let location_header = beam_resp
        .headers()
        .get(LOCATION)
        .unwrap()
        .to_str()
        .map_err(|e| {
            println!("Unable to parse location header received by beam-proxy: {}", e);
            (StatusCode::BAD_GATEWAY, "Unable to parse Beam.Proxy response")
        })
        .unwrap()
        .replace("tasks", "beam");

    let resp = (
           StatusCode::CREATED,
           [
               (LOCATION, location_header)
           ]
       );

    Ok(resp)
}

async fn listen_beam_results(args: &Arguments, task_id: String, wait_count: u16) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let client = Client::new();
    let url = 
        format!("{}v1/tasks/{}/results?wait_count={}", args.beam_url, task_id, wait_count)
        .parse::<Uri>()
        .expect("Unable to create URL query string, this should not happen.");

    let req = Request::builder()
        .method(Method::GET)
        .uri(url)
        .header(AUTHORIZATION, format!("ApiKey {}.torben-develop.broker.ccp-it.dktk.dkfz.de {}", args.beam_app, args.beam_secret))
        .header(ACCEPT, HeaderValue::from_static("text/event-stream"))
        .body(Body::empty())
        .expect("Unable to create HTTP query, this should not happen.");

    let mut resp = client
        .request(req)
        .await
        .map_err(|err| {
            println!("Failed request to {} with error: {}", args.beam_url, err.to_string());
            (StatusCode::BAD_GATEWAY, format!("Error calling beam, check the server logs."))
        })?;

    let code = resp.status();

    if ! code.is_success() {
        // How can i get the message from resp?
        return Err((code, format!("TODO: Proper handling of the error message here")))
    }

    let outgoing = async_stream::stream! {
        info!("Now creating outgoing sse stream");

        let incoming = resp
            .body_mut()
            .map_err(|e| futures::io::Error::new(futures::io::ErrorKind::Other, e));

        let mut decoder = async_sse::decode(incoming.into_async_read());

        while let Some(event) = decoder.next().await {
            let event = match event {
                Ok(event) => {
                    info!("Got event that is okay");
                    event
                },
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
                    info!("Got a retry message from broker");
                    format!("Got a retry message from the Broker, which is not yet supported.");
                },
                async_sse::Event::Message(event) => {
                    info!("Got a normal Message");
                    let event_type = SseEventType::from_str(event.name()).expect("Error in Infallible");
                    let event_as_bytes = event.into_bytes();
                    let event_as_str = std::str::from_utf8(&event_as_bytes).unwrap_or("(unable to parse)");
                    match &event_type {
                        SseEventType::DeletedTask | SseEventType::WaitExpired => {
                            debug!("SSE: Got {event_type} message, forwarding to App.");
                            yield Ok(Event::default()
                                .event(event_type)
                                .data(event_as_str));
                            continue;
                        },
                        SseEventType::Error => {
                            warn!("SSE: The Broker has reported an error: {event_as_str}");
                            yield Ok(Event::default()
                                .event(event_type)
                                .data(event_as_str));
                            continue;
                        },
                        SseEventType::Undefined => {
                            error!("SSE: Got a message without event type -- discarding.");
                            continue;
                        },
                        SseEventType::Unknown(s) => {
                            error!("SSE: Got unknown event type: {s} -- discarding.");
                            continue;
                        }
                        other => {
                            warn!("Got \"{other}\" event -- parsing.");
                        }
                    }
                    let as_string = std::str::from_utf8(&event_as_bytes).unwrap_or("(garbled_utf8)");
                    let event = Event::default()
                        .event(event_type)
                        .data(as_string);
                    yield Ok(event);
                }
            }
        }
    };

    let sse = Sse::new(outgoing);
    Ok(sse)
}

fn generate_auth_header (args: &Arguments) -> String {
    // TODO: Add Configuration for this
    // format!("ApiKey {}.dev-torben.broker.dev.ccp-it.dktk.dkfz.de {}", args.beam_app, args.beam_secret)
    format!("ApiKey {}.torben-develop.broker.ccp-it.dktk.dkfz.de {}", args.beam_app, args.beam_secret)
}


fn map_lens_to_beam(args: &Arguments, query: &LensQuery) -> BeamTask {
    let mut target_sites = Vec::new();
    for site in query.sites.clone() {
        // TODO: Configuration should also apply here
        target_sites.push(format!("focus.{}.broker.ccp-it.dktk.dkfz.de", site));
    }
    BeamTask {
        id: format!("{}", query.id),
        from: format!("{}.torben-develop.broker.ccp-it.dktk.dkfz.de", args.beam_app),
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
