//! The announced-key registry a pushed record's signature is checked against.
//!
//! A record's own `key_id` and `signature` prove that the holder of that key
//! signed that exact `capsule_id`. They do not prove who holds the key. This
//! registry is the other half: a map from a mesh peer id (its full endpoint
//! id) to the `key_id` that peer is known to sign with, so a push that
//! declares one peer but is signed with some other key is refused.
//!
//! It has two sources, merged by [`registry`]:
//!
//! - **The host's announced keys,** when the host lists the `plugin_keys.v1`
//!   capability: each peer's key for this plugin, bound to that peer by its
//!   own node key and checked by the host before it lists it at
//!   `GET /api/plugin-keys` ([`announced`]). Off a host without the
//!   capability, nothing is read.
//! - **The operator's map,** `CAPSULES_PEER_KEYS`: a JSON object mapping peer
//!   id to that peer's `key_id` (the raw Ed25519 public key, lowercase hex),
//!   e.g. `{"node-a": "3a1f…", "node-b": "9c02…"}`. It still works alone,
//!   and is the fallback for a peer whose host announces nothing.
//!
//! A peer absent from both is unknown. A peer the two sources give different
//! keys is "cannot verify" until the operator removes the stale entry: never
//! a choice between them. Every way the operator's value can fail (empty,
//! not JSON, not an object, the entry not a non-empty string) means "cannot
//! verify", never a guessed or default key.

/// The environment variable the registry is read from.
pub const ENV_PEER_KEYS: &str = "CAPSULES_PEER_KEYS";

/// The registry every check reads: the operator's map ([`ENV_PEER_KEYS`])
/// merged with the keys the host announced ([`announced::snapshot`]), as the
/// raw JSON object [`announced_key_in`] and [`peer_id_for_key_in`] take.
/// `None` when neither has anything.
pub fn registry() -> Option<String> {
    let configured = crate::settings::var(ENV_PEER_KEYS).ok();
    merged(configured.as_deref(), &announced::snapshot())
}

/// [`registry`] from its two sources. An operator value that is set but
/// unusable stays unusable (`Some` of itself, so every lookup is "cannot
/// verify"); a peer the two give different keys gets an empty key, which no
/// lookup accepts.
pub fn merged(
    configured: Option<&str>,
    announced: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let configured = configured.filter(|raw| !raw.is_empty());
    let mut map = match configured {
        None => serde_json::Map::new(),
        Some(raw) => match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(serde_json::Value::Object(map)) => map,
            _ => return Some(raw.to_string()),
        },
    };
    if configured.is_none() && announced.is_empty() {
        return None;
    }
    for (peer, key) in announced {
        let agreed = match map.get(peer) {
            None => key.clone(),
            Some(serde_json::Value::String(set)) if set == key => key.clone(),
            Some(_) => String::new(),
        };
        map.insert(peer.clone(), serde_json::Value::String(agreed));
    }
    Some(serde_json::Value::Object(map).to_string())
}

/// The keys the host announces for this plugin, read from the host's
/// `GET /api/plugin-keys` when it lists `plugin_keys.v1`.
pub mod announced {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, OnceLock, PoisonError};
    use std::time::Duration;

    /// The host capability that says it announces plugin keys.
    pub const CAPABILITY: &str = "plugin_keys.v1";
    /// The host's console API, where it lists announced keys. Unset:
    /// [`DEFAULT_CONSOLE_URL`].
    pub const ENV_CONSOLE_URL: &str = "CAPSULES_HOST_CONSOLE_URL";
    pub const DEFAULT_CONSOLE_URL: &str = "http://127.0.0.1:3131";
    const ROUTE: &str = "/api/plugin-keys";
    const EVERY: Duration = Duration::from_secs(30);
    const TIMEOUT: Duration = Duration::from_secs(5);

    fn keys() -> &'static Mutex<BTreeMap<String, String>> {
        static KEYS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
        KEYS.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    /// Peer id to this plugin's announced key, as last read. Empty off a
    /// host without the capability.
    pub fn snapshot() -> BTreeMap<String, String> {
        keys()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn is_key(key: &str) -> bool {
        key.len() == 64 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }

    /// Fold one `GET /api/plugin-keys` answer into `known`: this node's own
    /// key and each listed peer's key for `plugin`. A peer the answer lists
    /// without a key for `plugin` withdrew it, and is forgotten. A peer it
    /// does not list (no longer connected) keeps the key it announced: that
    /// binding was checked when it was received, and stays true.
    pub fn fold(known: &mut BTreeMap<String, String>, answer: &serde_json::Value, plugin: &str) {
        let mut listed: Vec<(&str, Option<&str>)> = Vec::new();
        if let Some(own) = answer.get("node_id").and_then(|v| v.as_str()) {
            listed.push((
                own,
                answer
                    .pointer(&format!("/plugin_keys/{plugin}"))
                    .and_then(|v| v.as_str()),
            ));
        }
        if let Some(peers) = answer.get("peers").and_then(|v| v.as_object()) {
            for (peer, keys) in peers {
                listed.push((peer.as_str(), keys.get(plugin).and_then(|v| v.as_str())));
            }
        }
        for (peer, key) in listed {
            if peer.is_empty() {
                continue;
            }
            match key.filter(|key| is_key(key)) {
                Some(key) => {
                    known.insert(peer.to_string(), key.to_string());
                }
                None => {
                    known.remove(peer);
                }
            }
        }
    }

    /// Start reading the host's announced keys, when `host_supports` says it
    /// has them; otherwise do nothing, and the operator's map alone applies.
    pub fn start(host_supports: bool, plugin: &'static str) {
        static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if !host_supports {
            tracing::info!("the host announces no plugin keys; CAPSULES_PEER_KEYS alone applies");
            return;
        }
        let base = crate::settings::var(ENV_CONSOLE_URL)
            .ok()
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| DEFAULT_CONSOLE_URL.to_string());
        let url = format!("{}{ROUTE}", base.trim_end_matches('/'));
        tokio::spawn(async move {
            let client = match reqwest::Client::builder().timeout(TIMEOUT).build() {
                Ok(client) => client,
                Err(error) => {
                    tracing::warn!(%error, "announced plugin keys are not read");
                    return;
                }
            };
            loop {
                match client.get(&url).send().await {
                    Ok(response) if response.status().is_success() => {
                        match response.json::<serde_json::Value>().await {
                            Ok(answer) => {
                                let mut known =
                                    keys().lock().unwrap_or_else(PoisonError::into_inner);
                                fold(&mut known, &answer, plugin);
                            }
                            Err(error) => {
                                tracing::debug!(%error, "unreadable announced plugin keys")
                            }
                        }
                    }
                    Ok(response) => {
                        tracing::debug!(status = %response.status(), "announced plugin keys not listed")
                    }
                    Err(error) => tracing::debug!(%error, "announced plugin keys not reachable"),
                }
                tokio::time::sleep(EVERY).await;
            }
        });
    }
}

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

/// The one announced peer whose key is `key_id` in `registry`: this node's
/// own id from its own key. `None` when no peer, or more than one, announces
/// it: never a guess.
pub fn peer_id_for_key_in(registry: Option<&str>, key_id: &str) -> Option<String> {
    if key_id.is_empty() {
        return None;
    }
    let raw = registry.filter(|raw| !raw.is_empty())?;
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    let mut named = parsed
        .as_object()?
        .iter()
        .filter(|(_, key)| key.as_str() == Some(key_id))
        .map(|(peer, _)| peer.clone());
    match (named.next(), named.next()) {
        (Some(peer), None) => Some(peer),
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

    #[test]
    fn a_key_names_a_peer_only_when_exactly_one_announces_it() {
        let registry = r#"{"node-a": "ab", "node-b": "cd", "node-c": "cd"}"#;
        assert_eq!(
            peer_id_for_key_in(Some(registry), "ab").as_deref(),
            Some("node-a")
        );
        assert_eq!(
            peer_id_for_key_in(Some(registry), "cd"),
            None,
            "two peers announce it"
        );
        assert_eq!(
            peer_id_for_key_in(Some(registry), "ef"),
            None,
            "nobody announces it"
        );
        assert_eq!(peer_id_for_key_in(Some(registry), ""), None, "no key");
        assert_eq!(peer_id_for_key_in(None, "ab"), None, "unset");
    }

    fn keys_of(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(p, k)| (p.to_string(), k.to_string()))
            .collect()
    }

    const KA: &str = "aa00000000000000000000000000000000000000000000000000000000000000";
    const KB: &str = "bb00000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn announced_keys_fill_in_beside_the_operator_map() {
        let registry = merged(
            Some(&format!(r#"{{"node-a": "{KA}"}}"#)),
            &keys_of(&[("node-b", KB)]),
        );
        assert_eq!(
            announced_key_in(registry.as_deref(), "node-a").as_deref(),
            Some(KA)
        );
        assert_eq!(
            announced_key_in(registry.as_deref(), "node-b").as_deref(),
            Some(KB)
        );
        let alone = merged(None, &keys_of(&[("node-b", KB)]));
        assert_eq!(
            announced_key_in(alone.as_deref(), "node-b").as_deref(),
            Some(KB),
            "no operator map"
        );
        assert_eq!(merged(None, &keys_of(&[])), None, "nothing from either");
    }

    #[test]
    fn two_sources_that_disagree_about_a_peer_verify_nothing_for_it() {
        let registry = merged(
            Some(&format!(r#"{{"node-a": "{KA}", "node-b": "{KB}"}}"#)),
            &keys_of(&[("node-a", KB), ("node-b", KB)]),
        );
        assert_eq!(
            announced_key_in(registry.as_deref(), "node-a"),
            None,
            "disagree"
        );
        assert_eq!(
            announced_key_in(registry.as_deref(), "node-b").as_deref(),
            Some(KB),
            "agree"
        );
        assert_eq!(
            peer_id_for_key_in(registry.as_deref(), KA),
            None,
            "the stale key names nobody"
        );
    }

    #[test]
    fn an_unusable_operator_map_stays_unusable_beside_announced_keys() {
        let registry = merged(Some("{node-a:"), &keys_of(&[("node-b", KB)]));
        assert_eq!(announced_key_in(registry.as_deref(), "node-b"), None);
    }

    #[test]
    fn the_hosts_answer_folds_in_own_and_peer_keys_for_this_plugin_only() {
        let mut known = keys_of(&[("gone", KA), ("withdrew", KA)]);
        let answer = serde_json::json!({
            "node_id": "self",
            "plugin_keys": {"capsules": KA, "other": KB},
            "peers": {
                "node-b": {"capsules": KB},
                "node-c": {"other": KB},
                "withdrew": {},
                "short": {"capsules": "ab"},
            }
        });
        announced::fold(&mut known, &answer, "capsules");
        assert_eq!(
            known,
            keys_of(&[("gone", KA), ("node-b", KB), ("self", KA)]),
            "own key in; a peer with another plugin's key only, a withdrawn key and a malformed key out; \
             a peer no longer listed keeps the key it announced"
        );
    }
}
