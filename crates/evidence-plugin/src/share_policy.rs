//! This plugin's `config_schema` declaration for
//! the one sharing-policy object -- four switches, one key
//! (`docs/SHARING-POLICY.md`). Declaring it here
//! makes mesh's console render the four switches under Configuration >
//! Plugins with the host's own controls -- nothing here is a score, and
//! nothing here ranks anyone.
//!
//! **Declarative only.** mesh-llm 0.76 gives a plugin its own declared
//! `config_schema` but there is no live host-to-plugin config channel wired
//! yet -- an operator sets the four `CAPSULE_EMIT_MESH_SHARE_*`
//! / `CAPSULE_EMIT_MESH_WITNESS` env vars directly on the node today. This
//! module's setting keys match those env var suffixes byte for byte
//! (`share_record_at_completion` -> `CAPSULE_EMIT_MESH_SHARE_RECORD_AT_COMPLETION`,
//! etc.) so wiring the host's resolved value into this process's env later
//! is a direct 1:1 map, not a re-naming exercise. Adding this schema does
//! not by itself flip any runtime behavior -- it only makes the four
//! switches visible and editable in the console.
//!
//! The same schema also declares the opt-in stop-routing rule's two settings
//! (`routing_rule`), under their own "Routing rule" category, on the same
//! env-var convention: `CAPSULE_EMIT_MESH_STOP_ROUTING_AFTER_CONTRADICTIONS`
//! (off unless set) and `CAPSULE_EMIT_MESH_STOP_ROUTING_WINDOW_DAYS`.

use mesh_llm_plugin::{
    config_enum, config_integer, config_schema, config_setting, config_url, ManifestEntry,
};

pub const RECORD_AT_COMPLETION_KEY: &str = "share_record_at_completion";
pub const HISTORY_SEGMENTS_KEY: &str = "share_history_segments";
pub const ADJUDICATIONS_KEY: &str = "share_adjudications";
pub const WITNESS_KEY: &str = "witness";
/// The opt-in stop-routing rule (`routing_rule`): N, and D in days. Same
/// env-suffix convention (`CAPSULE_EMIT_MESH_STOP_ROUTING_*`).
pub const STOP_ROUTING_AFTER_KEY: &str = "stop_routing_after_contradictions";
pub const STOP_ROUTING_WINDOW_KEY: &str = "stop_routing_window_days";

/// This process's own runtime env var for `share_record_at_completion` --
/// see the module doc's "declarative only" note: no live host->plugin
/// config channel exists yet, so an operator sets this directly on the
/// node.
pub const ENV_RECORD_AT_COMPLETION: &str = "CAPSULE_EMIT_MESH_SHARE_RECORD_AT_COMPLETION";

/// This process's runtime `share_record_at_completion` value: `true` only
/// when explicitly set to `"off"`; unset or any other value resolves to the
/// documented default (`"counterparty"`, i.e. NOT off).
/// Seam A1.
pub fn record_at_completion_is_off() -> bool {
    record_at_completion_is_off_for(
        crate::settings::var(ENV_RECORD_AT_COMPLETION)
            .ok()
            .as_deref(),
    )
}

fn record_at_completion_is_off_for(raw: Option<&str>) -> bool {
    raw == Some("off")
}

/// This process's own env var for `share_history_segments`.
pub const ENV_HISTORY_SEGMENTS: &str = "CAPSULE_EMIT_MESH_SHARE_HISTORY_SEGMENTS";

/// Who may read one of this node's records back: `off`, `counterparties`,
/// `prospective` or `peers`. Unset or unknown is the documented default,
/// `prospective`.
pub fn history_segments() -> &'static str {
    history_segments_for(crate::settings::var(ENV_HISTORY_SEGMENTS).ok().as_deref())
}

fn history_segments_for(raw: Option<&str>) -> &'static str {
    match raw {
        Some("off") => "off",
        Some("counterparties") => "counterparties",
        Some("peers") => "peers",
        _ => "prospective",
    }
}

const CATEGORY_ID: &str = "share";
const CATEGORY_LABEL: &str = "Sharing policy";
const CATEGORY_SUMMARY: &str = "What this node shares, with whom, by default. Every default keys \
    on relationship (counterparty in the window), never proximity or latency.";

const RULE_CATEGORY_ID: &str = "routing_rule";
const RULE_CATEGORY_LABEL: &str = "Routing rule";
const RULE_CATEGORY_SUMMARY: &str =
    "An optional rule of yours for when this node stops routing to a \
    peer. Off by default.";

/// The `config_schema` manifest entry naming all four switches. Attach with
/// `DeclarativePluginBuilder::config_item`.
pub fn share_policy_config_schema(plugin_id: &str) -> ManifestEntry {
    config_schema(plugin_id)
        .setting(
            config_setting(RECORD_AT_COMPLETION_KEY, config_enum(["counterparty", "off"]))
                .default_value(&"counterparty")
                .description(
                    "Push this node's own sealed record of a completed exchange to the \
                     counterparty (counterparty), or don't (off). With checkpointing on, the \
                     push also carries a signed checkpoint of this node's log and the record's \
                     inclusion proof -- the checkpoint reveals the log's total size (records \
                     across all peers) and its time, never any other record's content. \
                     Symmetric: a node with this off also does not receive the other side's \
                     push -- fetch-on-request still works either way.",
                )
                .label("Record at completion")
                .category(CATEGORY_ID, CATEGORY_LABEL, CATEGORY_SUMMARY, 0),
        )
        .setting(
            config_setting(
                HISTORY_SEGMENTS_KEY,
                config_enum(["counterparties", "prospective", "peers", "off"]),
            )
            .default_value(&"prospective")
            .description(
                "Who this node answers a chain-segment/record/correlation request from: past \
                 counterparties only, counterparties plus prospective ones (default), any peer, \
                 or nobody. A refusal is structural (not_authorized), never a score.",
            )
            .label("History segments")
            .category(CATEGORY_ID, CATEGORY_LABEL, CATEGORY_SUMMARY, 1),
        )
        .setting(
            config_setting(ADJUDICATIONS_KEY, config_enum(["deliver_to_subjects", "off"]))
                .default_value(&"deliver_to_subjects")
                .description(
                    "Deliver a sealed verdict to every node it judges (default), or don't. The \
                     subject acks or disputes on its own chain -- this is delivery, never a \
                     computed standing.",
                )
                .label("Adjudications")
                .category(CATEGORY_ID, CATEGORY_LABEL, CATEGORY_SUMMARY, 2),
        )
        .setting(
            config_setting(WITNESS_KEY, config_url())
                .description(
                    "Witness service URL to checkpoint against. Empty is off (the default) -- no \
                     default witness URL, no network call without an operator-supplied one.",
                )
                .label("Witness")
                .category(CATEGORY_ID, CATEGORY_LABEL, CATEGORY_SUMMARY, 3),
        )
        .setting(
            config_setting(STOP_ROUTING_AFTER_KEY, config_integer())
                .description(
                    "Stop routing to a peer after this many contradictions within the window below. \
                     Off when empty or 0 (the default). Only verdicts this node asked a referee \
                     for, and whose signature it checked, count, once per referee and pair of \
                     answers; with N of 2 or more, no single referee can reach N alone. When it fires, the host blocks the peer until you undo \
                     it, exactly like Stop routing, and the sealed record names this rule and \
                     cites the verdicts. Undo it the same way. Nothing is scored or sent.",
                )
                .label("Stop routing after N contradictions")
                .category(RULE_CATEGORY_ID, RULE_CATEGORY_LABEL, RULE_CATEGORY_SUMMARY, 0),
        )
        .setting(
            config_setting(STOP_ROUTING_WINDOW_KEY, config_integer())
                .default_value(&30)
                .description("The window, in days, the contradictions must fall in (default 30).")
                .label("Within D days")
                .category(RULE_CATEGORY_ID, RULE_CATEGORY_LABEL, RULE_CATEGORY_SUMMARY, 1),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesh_llm_plugin::proto;

    fn as_config_schema(entry: ManifestEntry) -> proto::PluginConfigSchemaManifest {
        match entry {
            ManifestEntry::ConfigSchema(schema) => schema,
            other => panic!("expected ManifestEntry::ConfigSchema, got {other:?}"),
        }
    }

    #[test]
    fn declares_all_four_switches_under_the_plugin_id() {
        let schema = as_config_schema(share_policy_config_schema("capsule-emit-mesh"));
        assert_eq!(schema.plugin_name, "capsule-emit-mesh");
        let keys: Vec<&str> = schema.settings.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                RECORD_AT_COMPLETION_KEY,
                HISTORY_SEGMENTS_KEY,
                ADJUDICATIONS_KEY,
                WITNESS_KEY,
                STOP_ROUTING_AFTER_KEY,
                STOP_ROUTING_WINDOW_KEY
            ]
        );
    }

    #[test]
    fn history_segments_defaults_to_prospective_and_reads_the_four_tiers() {
        assert_eq!(history_segments_for(None), "prospective");
        assert_eq!(history_segments_for(Some("bogus")), "prospective");
        for tier in ["off", "counterparties", "prospective", "peers"] {
            assert_eq!(history_segments_for(Some(tier)), tier);
        }
    }

    #[test]
    fn setting_keys_match_the_python_env_var_suffix_convention() {
        // Each env var is the setting key with the shared
        // CAPSULE_EMIT_MESH_ prefix -- this module's whole "no
        // re-naming exercise later" claim rests on this correspondence.
        let expected_env_suffix = [
            (RECORD_AT_COMPLETION_KEY, "SHARE_RECORD_AT_COMPLETION"),
            (HISTORY_SEGMENTS_KEY, "SHARE_HISTORY_SEGMENTS"),
            (ADJUDICATIONS_KEY, "SHARE_ADJUDICATIONS"),
            (WITNESS_KEY, "WITNESS"),
            (STOP_ROUTING_AFTER_KEY, "STOP_ROUTING_AFTER_CONTRADICTIONS"),
            (STOP_ROUTING_WINDOW_KEY, "STOP_ROUTING_WINDOW_DAYS"),
        ];
        for (key, suffix) in expected_env_suffix {
            assert_eq!(
                key.to_uppercase(),
                suffix,
                "key {key} must upper-case to its env suffix"
            );
        }
    }

    #[test]
    fn record_at_completion_and_adjudications_default_to_the_documented_on_state() {
        let schema = as_config_schema(share_policy_config_schema("capsule-emit-mesh"));
        let by_key = |k: &str| schema.settings.iter().find(|s| s.key == k).unwrap();
        assert_eq!(
            by_key(RECORD_AT_COMPLETION_KEY).default_json.as_deref(),
            Some("\"counterparty\"")
        );
        assert_eq!(
            by_key(HISTORY_SEGMENTS_KEY).default_json.as_deref(),
            Some("\"prospective\"")
        );
        assert_eq!(
            by_key(ADJUDICATIONS_KEY).default_json.as_deref(),
            Some("\"deliver_to_subjects\"")
        );
    }

    #[test]
    fn the_stop_routing_rule_is_off_by_default() {
        let schema = as_config_schema(share_policy_config_schema("capsule-emit-mesh"));
        let after = schema
            .settings
            .iter()
            .find(|s| s.key == STOP_ROUTING_AFTER_KEY)
            .unwrap();
        assert_eq!(after.default_json, None, "no N by default: the rule is off");
        assert_eq!(
            format!(
                "CAPSULE_EMIT_MESH_{}",
                STOP_ROUTING_AFTER_KEY.to_uppercase()
            ),
            crate::routing_rule::ENV_AFTER
        );
        assert_eq!(
            format!(
                "CAPSULE_EMIT_MESH_{}",
                STOP_ROUTING_WINDOW_KEY.to_uppercase()
            ),
            crate::routing_rule::ENV_WINDOW_DAYS
        );
    }

    #[test]
    fn witness_has_no_default_url_and_is_not_required() {
        // Design note S1: "witness: off | <url> # default: off" -- off IS
        // the absence of a default, never a magic sentinel string.
        let schema = as_config_schema(share_policy_config_schema("capsule-emit-mesh"));
        let witness = schema
            .settings
            .iter()
            .find(|s| s.key == WITNESS_KEY)
            .unwrap();
        assert_eq!(witness.default_json, None);
        assert!(!witness.required);
    }

    #[test]
    fn every_setting_is_optional_shipping_this_schema_flips_no_runtime_behavior() {
        let schema = as_config_schema(share_policy_config_schema("capsule-emit-mesh"));
        assert!(schema.settings.iter().all(|s| !s.required));
    }

    #[test]
    fn record_at_completion_is_off_only_for_the_explicit_off_value() {
        assert!(record_at_completion_is_off_for(Some("off")));
    }

    #[test]
    fn record_at_completion_defaults_on_when_unset() {
        assert!(!record_at_completion_is_off_for(None));
    }

    #[test]
    fn record_at_completion_defaults_on_for_any_other_value() {
        assert!(!record_at_completion_is_off_for(Some("counterparty")));
        assert!(!record_at_completion_is_off_for(Some("garbage")));
    }
}
