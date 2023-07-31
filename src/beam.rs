use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct BeamTask {
    pub id: String,
    from: String,
    to: Vec<String>,
    metadata: String,
    body: String,
    failure_strategy: FailureStrategy,
    ttl: String,
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

impl BeamTask {
    pub fn new(
        id: String,
        target_application: String,
        target_sites: Vec<String>,
        query: String,
    ) -> BeamTask {
        let mut transformed_sites = Vec::new();
        for site in target_sites.clone() {
            // TODO: Configuration should also apply here
            transformed_sites.push(format!("focus.{}.broker.ccp-it.dktk.dkfz.de", site));
        }
        BeamTask {
            id,
            from: format!(
                "{}.torben-develop.broker.ccp-it.dktk.dkfz.de",
                target_application
            ),
            to: target_sites,
            metadata: format!(""),
            body: query,
            failure_strategy: FailureStrategy::Retry(Retry {
                backoff_millisecs: 1000,
                max_tries: 5,
            }),
            ttl: format!("360s"),
        }
    }
}
