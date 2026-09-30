//! The announced-key registry a pushed record's signature is checked against.
//!
//! A record's own `key_id` and `signature` prove that the holder of that key
//! signed that exact `capsule_id`. They do not prove who holds the key. This
//! registry is the other half: an operator-configured map from a mesh peer id
//! to the `key_id` that peer is known to sign with, so a push that declares
//! one peer but is signed with some other key is refused.
//!
//! **Config, not discovery.** The operator sets `CAPSULE_EMIT_MESH_PEER_KEYS`
//! on the node to a JSON object mapping peer id to that peer's `key_id` (the
//! raw Ed25519 public key, lowercase hex), e.g. `{"node-a": "3a1f…", "node-b":
//! "9c02…"}`. A peer absent from the map is unknown. Every way the value can
//! fail (unset, empty, not JSON, not an object, the entry not a non-empty
//! string) means "cannot verify", never a guessed or default key.

/// The environment variable the registry is read from.
pub const ENV_PEER_KEYS: &str = "CAPSULE_EMIT_MESH_PEER_KEYS";

/// The `key_id` configured for `peer_id` in `registry`, the raw value of
/// [`ENV_PEER_KEYS`] (`None` when it is unset). `None` whenever the answer is
/// not a non-empty string: see the module doc.
pub fn announced_key_in(registry: Option<&str>, peer_id: &str) -> Option<String> {
    if peer_id.is_empty() {
        return None;
    }
    let raw = registry.filter(|raw| !raw.is_empty())?;
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    match parsed.as_object()?.get(peer_id)? {
        serde_json::Value::String(key_id) if !key_id.is_empty() => Some(key_id.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_configured_non_empty_string_is_an_announced_key() {
        let registry = r#"{"node-a": "ab", "node-b": "", "node-c": 7}"#;
        assert_eq!(
            announced_key_in(Some(registry), "node-a").as_deref(),
            Some("ab")
        );
        assert_eq!(
            announced_key_in(Some(registry), "node-b"),
            None,
            "empty key"
        );
        assert_eq!(
            announced_key_in(Some(registry), "node-c"),
            None,
            "not a string"
        );
        assert_eq!(announced_key_in(Some(registry), "m9"), None, "unknown peer");
        assert_eq!(announced_key_in(Some(registry), ""), None, "no peer id");
        assert_eq!(announced_key_in(None, "node-a"), None, "unset");
        assert_eq!(announced_key_in(Some(""), "node-a"), None, "empty");
        assert_eq!(
            announced_key_in(Some("{node-a:"), "node-a"),
            None,
            "not JSON"
        );
        assert_eq!(
            announced_key_in(Some(r#"["ab"]"#), "node-a"),
            None,
            "not an object"
        );
    }
}
