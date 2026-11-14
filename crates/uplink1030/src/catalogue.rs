//! Catalogue evidence is displayed as hypotheses; never used to assign a radar identity.
use crate::{activity::valid_code, source::SourceError};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn load(path: &str) -> Result<BTreeMap<String, Vec<String>>, SourceError> {
    let data: Value =
        serde_json::from_reader(std::fs::File::open(shellexpand::tilde(path).as_ref())?)?;
    parse(&data).map_err(Into::into)
}
fn parse(data: &Value) -> Result<BTreeMap<String, Vec<String>>, String> {
    let sites = data["sites"]
        .as_array()
        .ok_or("catalogue requires a sites array")?;
    let assignments = data["code_assignment_evidence"]
        .as_array()
        .ok_or("catalogue requires code_assignment_evidence array")?;
    let locations = data["location_evidence"]
        .as_array()
        .ok_or("catalogue requires location_evidence array")?;
    let mut result = BTreeMap::<String, BTreeMap<String, String>>::new();
    for a in assignments {
        if a["syntax_valid"].as_bool() != Some(true) {
            continue;
        }
        let code = format!("{}{}", a["kind"].as_str().unwrap_or(""), a["value"]);
        if !valid_code(&code) {
            continue;
        }
        let Some(site) = sites
            .iter()
            .find(|s| s["site_id"].as_str().is_some() && s["site_id"] == a["site_id"])
        else {
            continue;
        };
        let name = site["name"].as_str().unwrap_or("unnamed candidate");
        let positions: Vec<_> = locations
            .iter()
            .filter(|l| l["site_id"] == a["site_id"])
            .map(|l| {
                format!(
                    "{} ({})",
                    l["coordinates"],
                    l["status"].as_str().unwrap_or("unreviewed")
                )
            })
            .collect();
        let text = format!(
            "{name} | locations: {} | assignment: {}, verified={} | validity {}..{} | source {}",
            if positions.is_empty() {
                "unknown".into()
            } else {
                positions.join("; ")
            },
            a["status"].as_str().unwrap_or("unknown"),
            a["assignment_verified"].as_bool().unwrap_or(false),
            a["valid_from"],
            a["valid_to"],
            a["source_record_id"].as_str().unwrap_or("unknown")
        );
        let claims = result.entry(code).or_default();
        let site_id = site["site_id"].as_str().unwrap().to_owned();
        claims
            .entry(site_id)
            .and_modify(|previous| {
                if previous != &text {
                    previous.push_str("\nAdditional assignment evidence: ");
                    previous.push_str(&text);
                }
            })
            .or_insert(text);
    }
    Ok(result
        .into_iter()
        .map(|(code, sites)| (code, sites.into_values().collect()))
        .collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn reused_codes_keep_all_candidates_and_missing_locations() {
        let data = json!({"sites":[{"site_id":"a","name":"Alpha"},{"site_id":"b","name":"Beta"}],"location_evidence":[],"code_assignment_evidence":[
            {"site_id":"a","kind":"II","value":3,"syntax_valid":true,"status":"historical_source_claim","assignment_verified":false},
            {"site_id":"b","kind":"II","value":3,"syntax_valid":true,"status":"undated_catalogue_alias_claim","assignment_verified":false}
        ]});
        let map = parse(&data).unwrap();
        assert_eq!(map["II3"].len(), 2);
        assert!(map["II3"][0].contains("locations: unknown"));
        assert!(map["II3"][1].contains("verified=false"));
        assert!(parse(&json!({})).is_err());
    }
}
