use std::sync::Arc;

use axum::{Router, routing::post, extract::{Json, State}};
use clap::Parser;
use reqwest::{Response, Url};
use serde::{Serialize, Deserialize};
use uuid::Uuid;

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

#[derive(Serialize, Deserialize)]
struct BeamTask {
    id: String,
    from: String,
    to: Vec<String>,
    metadata: String,
    body: String,
    // TODO: Failure Strategy
    failure_strategy: FailureStrategy,
    ttl: String,
}

async fn handle_post(
    State(args): State<Arc<Arguments>>,
    Json(query): Json<LensQuery>
) -> String {
    let result = create_beam_task(&args, &query).await;
    match result {
        Ok(data) => format!("The response has status {}", data.status()),
        Err(err) => format!("The error says: {}", err.to_string())
    }
}


#[tokio::main]
async fn main() {
    let args = Arc::new(Arguments::parse());
    println!("Beam URL: {}", &args.beam_url);
    println!("Beam App: {}", &args.beam_app);
    // TODO: Add check for reachability of beam-proxy
    let app = Router::new()
        .route("/", post(handle_post))
        .with_state(args);

    axum::Server::bind(&"0.0.0.0:8080".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn create_beam_task(args: &Arguments, query: &LensQuery) -> Result<Response, reqwest::Error> {
    let client = reqwest::Client::new();
    let url = format!("{}v1/tasks", args.beam_url);
    let auth_header = format!("ApiKey {}.dev-torben.broker.dev.ccp-it.dktk.dkfz.de {}", args.beam_app, args.beam_secret);
    let body = map_lens_to_beam(args, query);
    println!("{}", url);
    println!("{}", auth_header);
    println!("{}", body.id);
    client.post(url)
        .header("Authorization", auth_header)
        .json(&body)
        .send()
        .await
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
