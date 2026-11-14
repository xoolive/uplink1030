//! Beta output normalization; no versioning or deduplication.

use serde_json::{Map, Value};

/// Move reception measurements to one observation. For ACAS broadcasts only,
/// identify the transmitting aircraft by MID rather than the broadcast AP.
/// Resolution messages keep their addressed target and separate MID sender.
pub fn normalize_output(message: &mut Map<String, Value>) {
    let mut observation = Map::new();
    for key in ["rssi", "confidence"] {
        if let Some(value) = message.remove(key) {
            observation.insert(key.into(), value);
        }
    }
    if !observation.is_empty() {
        message
            .entry("metadata")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("output metadata is an array")
            .push(observation.into());
    }
    if message.get("uf").and_then(Value::as_str) != Some("16")
        || message.get("kind").and_then(Value::as_str) != Some("acas_b")
        || !message.contains_key("mid")
    {
        return;
    }
    let mid = message.remove("mid").unwrap();
    if let Some(ap) = message.remove("icao24") {
        // Preserve the actual AP field, including its broadcast value. It is
        // no longer presented as the identity of an aircraft.
        message.insert("ap".into(), ap);
    }
    // `known` was evaluated against AP, not against this sender's MID.
    message.remove("known");
    if let Some(sender) = mid.as_str().filter(|address| {
        address.len() == 6
            && address.bytes().all(|c| c.is_ascii_hexdigit())
            && !address.eq_ignore_ascii_case("ffffff")
            && *address != "000000"
    }) {
        message.insert("icao24".into(), sender.to_ascii_lowercase().into());
        message.insert("address_role".into(), "sender".into());
    }
}

/// Activity can retain an unidentified broadcast under its actual AP address
/// without putting that broadcast address back into the JSON aircraft identity.
pub fn activity_address(message: &Value) -> Option<&str> {
    message["icao24"].as_str().or_else(|| {
        if message["uf"] == "16" && message["kind"] == "acas_b" {
            message["ap"].as_str()
        } else {
            None
        }
    })
}

/// UI/replay access accepts both old recordings and the new observation layout.
pub fn observation<'a>(message: &'a Value, key: &str) -> &'a Value {
    message
        .get("metadata")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get(key))
        .unwrap_or(&message[key])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn normalized(mut value: Value) -> Value {
        normalize_output(value.as_object_mut().unwrap());
        value
    }

    #[test]
    fn measurements_move_to_one_observation_without_a_version_or_merging() {
        let value =
            normalized(json!({"uf":"4", "icao24":"392aed", "rssi":-25.0, "confidence":0.95}));
        assert_eq!(
            value["metadata"],
            json!([{"rssi":-25.0, "confidence":0.95}])
        );
        assert!(value.get("rssi").is_none());
        assert!(value.get("confidence").is_none());
        assert!(value.get("v").is_none());
        assert_eq!(value["icao24"], "392aed");
        assert_eq!(normalized(value.clone()), value);
    }

    #[test]
    fn acas_broadcast_uses_sender_and_preserves_ap_without_a_fake_aircraft() {
        let value = normalized(
            json!({"uf":"16", "kind":"acas_b", "icao24":"ffffff", "mid":"4B1804", "known":true}),
        );
        assert_eq!(value["icao24"], "4b1804");
        assert_eq!(value["ap"], "ffffff");
        assert_eq!(value["address_role"], "sender");
        assert!(value.get("mid").is_none());
        assert!(value.get("known").is_none());
        assert_eq!(normalized(value.clone()), value);
    }

    #[test]
    fn resolution_keeps_target_and_mid_sender_distinct() {
        let value =
            normalized(json!({"uf":"16", "kind":"resolution", "icao24":"392aed", "mid":"4b1804"}));
        assert_eq!(value["icao24"], "392aed");
        assert_eq!(value["mid"], "4b1804");
        assert!(value.get("ap").is_none());
    }

    #[test]
    fn reserved_sender_is_not_promoted_as_aircraft_identity() {
        for sender in ["ffffff", "000000", "bad"] {
            let value =
                normalized(json!({"uf":"16", "kind":"acas_b", "icao24":"ffffff", "mid":sender}));
            assert!(value.get("icao24").is_none());
            assert_eq!(value["ap"], "ffffff");
            assert_eq!(activity_address(&value), Some("ffffff"));
        }
    }

    #[test]
    fn observation_reader_accepts_both_layouts() {
        let old = json!({"rssi":-20.0, "confidence":0.9});
        let new = normalized(old.clone());
        assert_eq!(observation(&old, "rssi"), observation(&new, "rssi"));
        assert_eq!(
            observation(&old, "confidence"),
            observation(&new, "confidence")
        );
        assert!(observation(&new, "missing").is_null());
    }
}
