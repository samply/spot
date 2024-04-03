use std::time::Duration;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
enum ChildCategoryType {
    Equals,
    SomethingElse
}

#[derive(Serialize, Deserialize)]
struct ChildCategoryCriterion {
    key: String,
    count: Option<usize>
}

#[derive(Serialize, Deserialize)]
struct ChildCategory {
    key: String,
    type_: ChildCategoryType,
    criteria: Vec<ChildCategoryCriterion>
}

pub async fn get_extended_json(catalogue_url: Url) -> Value {
    debug!("Fetching catalogue from {catalogue_url} ...");

    let resp = reqwest::Client::new()
    .get(catalogue_url)
    .timeout(Duration::from_secs(30))
    .send()
    .await
    .expect("Unable to fetch catalogue from upstream; please check URL specified in config.");

    let mut json: Value = resp.json().await
        .expect("Unable to parse catalogue from upstream; please check URL specified in config.");

    // TODO: Query prism for counts here.

    recurse(&mut json);

    // println!("{}", serde_json::to_string_pretty(&json).unwrap());

    info!("Catalogue built successfully.");

    json
}

/// Key order: group key (e.g. patient)
///            \-- stratifier key (e.g. admin_gender)
///                \-- stratum key (e.g. male, other)
fn recurse(json: &mut Value) {
    match json {
        Value::Null => (),
        Value::Bool(_) => (),
        Value::Number(_) => (),
        Value::String(_) => (),
        Value::Array(arr) => {
            for ele in arr {
                recurse(ele);
            }
        },
        Value::Object(obj) => {
            if ! obj.contains_key("childCategories") {
                for (_key, child_val) in obj.iter_mut() {
                    recurse(child_val);
                }
            } else {
                let group_key = obj.get("key").expect("Got JSON element with childCategories but without (group) key. Please check json.");

                let children_cats = obj
                    .get_mut("childCategories")
                    .unwrap()
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .filter(|item| item.get("type").unwrap_or(&Value::Null) == "EQUALS");
                
                for child_cat in children_cats {
                    let stratifier_key = child_cat.get("key").expect("Got JSON element with childCategory that does not contain a (stratifier) key. Please check json.");
                    let criteria = child_cat
                        .get_mut("criteria")
                        .expect("Got JSON element with childCategory that does not contain a criteria array. Please check json.")
                        .as_array_mut()
                        .expect("Got JSON element with childCategory with criteria that are not an array. Please check json.");

                    for criterion in criteria {
                        let criterion = criterion.as_object_mut()
                            .expect("Got JSON where a criterion was not an object. Please check json.");
                        let stratum_key = criterion.get("key")
                            .expect("Got JSON where a criterion did not have a key. Please check json.")
                            .as_str()
                            .expect("Got JSON where a criterion key was not a string. Please check json.");
                        
                        // fetch from Prism output
                        let count_from_prism = 10;

                        criterion.insert("count".into(), json!(count_from_prism));
                    }
                }
            }
        },
    }
}