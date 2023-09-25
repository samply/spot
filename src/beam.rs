use beam_lib::{TaskRequest, AppId};
use crate::CONFIG;

pub fn create_beam_task(
    id: beam_lib::MsgId,
    target_sites: Vec<AppId>,
    query: String,
) -> TaskRequest<String> {
    let mut transformed_sites = Vec::new();
    for site in target_sites.clone() {
        // TODO: Configuration should also apply here
        transformed_sites.push(format!("focus.{}.broker.ccp-it.dktk.dkfz.de", site));
    }
    TaskRequest {
        id,
        from: CONFIG.beam_app.clone(),
        to: target_sites,
        metadata: serde_json::Value::Null,
        body: query,
        failure_strategy: beam_lib::FailureStrategy::Retry {
            backoff_millisecs: 1000,
            max_tries: 5,
        },
        ttl: "360s".to_string(),
    }
}
