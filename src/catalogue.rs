use std::{collections::BTreeMap, time::Duration};

use reqwest::Url;
use serde_json::{json, Value};
use tracing::{debug, info};

pub type Criteria = BTreeMap<String, u64>;

pub type CriteriaGroup = BTreeMap<String, Criteria>;

pub type CriteriaGroups = BTreeMap<String, CriteriaGroup>;

fn get_element<'a>(count: &'a CriteriaGroups, key1: &'a str, key2: &'a str, key3: &'a str) -> Option<&'a u64> {
    count.get(key1)
        .and_then(|group| group.get(key2))
        .and_then(|criteria| criteria.get(key3))
}


pub async fn get_extended_json(catalogue_url: Url, prism_url: Url) -> Value {
    debug!("Fetching catalogue from {catalogue_url} ...");

    let resp = reqwest::Client::new()
        .get(catalogue_url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .expect("Unable to fetch catalogue from upstream; please check URL specified in config.");

    let mut json: Value = resp.json().await
        .expect("Unable to parse catalogue from upstream; please check URL specified in config.");

//    tokio::time::sleep(Duration::from_secs(10)).await;    

    let prism_resp = reqwest::Client::new()
        .post(format!("{}criteria", prism_url))
        .header("Content-Type", "application/json")
        .body("{\"sites\": []}")
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .expect("Unable to fetch response from Prism; please check it's running.");

    let mut counts: CriteriaGroups = prism_resp.json().await
        .expect("Unable to parse response from Prism into CriteriaGroups");

    recurse(&mut json, &mut counts); //TODO remove from counts once copied into catalogue to make it O(n log n)

    info!("Catalogue built successfully.");

    json
}

/// Key order: group key (e.g. patient)
///            \-- stratifier key (e.g. admin_gender)
///                \-- stratum key (e.g. male, other)
fn recurse(json: &mut Value, counts: &mut CriteriaGroups) {
    match json {
        Value::Array(arr) => {
            for ele in arr {
                recurse(ele, counts);
            }
        },
        Value::Object(obj) => {
            
            if ! obj.contains_key("childCategories") {
                for (_key, child_val) in obj.iter_mut() {
                    recurse(child_val, counts);
                }
            } else {
                let group_key = obj.get("key").expect("Got JSON element with childCategories but without (group) key. Please check json.").as_str()
                .expect("Got JSON where a criterion key was not a string. Please check json.").to_owned();

                //TODO consolidate catalogue and MeasureReport group names
                let group_key = if group_key == "patient" {"patients"} 
                else if group_key == "tumor_classification" {"diagnosis"} 
                else if group_key == "biosamples" {"specimen"}
                else {&group_key};

                let children_cats = obj
                    .get_mut("childCategories")
                    .unwrap()
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .filter(|item| item.get("type").unwrap_or(&Value::Null) == "EQUALS");
                
                for child_cat in children_cats {
                    let stratifier_key = child_cat.get("key").expect("Got JSON element with childCategory that does not contain a (stratifier) key. Please check json.").as_str()
                    .expect("Got JSON where a criterion key was not a string. Please check json.").to_owned();

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
                        
                        let count_from_prism = get_element(counts, &group_key, &stratifier_key, stratum_key);

                        match count_from_prism {
                            Some(count) => {
                                criterion.insert("count".into(), json!(count));
                            },
                            None => {
                                debug!("No count from Prism for {}, {}, {}", group_key, stratifier_key, stratum_key);
                            }
                        }                        
                    }
                }
            }
        },
        _ => {}
    }
}