use beam_lib::{TaskRequest, AppId, RawString};
use crate::CONFIG;
use tracing::{info, warn, Level};

pub fn create_beam_task(
    id: beam_lib::MsgId,
    target_sites: Vec<String>,
    query: String,
) -> TaskRequest<RawString> {
    let proxy_id = CONFIG.beam_app_id.proxy_id();
    let broker_id = proxy_id.as_ref().split_once('.').expect("Invalid beam id in config").1;
    let to = target_sites.into_iter().map(|site| AppId::new_unchecked(format!("focus.{site}.{broker_id}"))).collect();
    let metadata = if let Some(project) = &CONFIG.project {
        serde_json::json!({
            "project": project
        })
    } else {
        serde_json::Value::Null
    };

    // Print the query
    info!("create_beam_task: running query");

    TaskRequest {
        id,
        from: CONFIG.beam_app_id.clone(),
        to,
        metadata,
        body: query.into(),
        failure_strategy: beam_lib::FailureStrategy::Retry {
            backoff_millisecs: 1000,
            max_tries: 5,
        },
        ttl: "360s".to_string(),
    }
}
