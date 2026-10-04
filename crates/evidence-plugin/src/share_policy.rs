//! This plugin's `config_schema` declaration for
//! the one sharing-policy object -- four switches, one key
//! (`docs/SHARING-POLICY.md`). Declaring it here
//! makes mesh's console render the four switches under Configuration >
//! Plugins with the host's own controls -- nothing here is a score, and
//! nothing here ranks anyone.
//!
//! **How a console value reaches the plugin.** mesh-llm saves what the
//! operator sets in the console to its config file (this plugin's
//! `[plugin.settings]`) and does not pass it to the plugin process (no env,
//! no protocol message). `crate::settings::var` reads it there when the
//! environment does not set the setting. Each key is its env var's suffix
//! lower-cased (`share_record_at_completion` ->
//! `CAPSULES_SHARE_RECORD_AT_COMPLETION`), except `witness`, which
//! is `CAPSULES_CHECKPOINT_WITNESS_URLS`. The witness is read when
//! the plugin starts: restart mesh-llm after changing it.
//!
//! The same schema also declares the opt-in stop-routing rule's two settings
//! (`routing_rule`), under their own "Routing rule" category, on the same
//! env-var convention: `CAPSULES_STOP_ROUTING_AFTER_CONTRADICTIONS`
//! (off unless set) and `CAPSULES_STOP_ROUTING_WINDOW_DAYS`.

use mesh_llm_plugin::{
    config_array, config_enum, config_integer, config_schema, config_setting, config_url,
    ManifestEntry,
};

pub const RECORD_AT_COMPLETION_KEY: &str = "share_record_at_completion";
pub const HISTORY_SEGMENTS_KEY: &str = "share_history_segments";
pub const ADJUDICATIONS_KEY: &str = "share_adjudications";
pub const WITNESS_KEY: &str = "witness";
/// The opt-in stop-routing rule (`routing_rule`): N, and D in days. Same
/// env-suffix convention (`CAPSULES_STOP_ROUTING_*`).
pub const STOP_ROUTING_AFTER_KEY: &str = "stop_routing_after_contradictions";
pub const STOP_ROUTING_WINDOW_KEY: &str = "stop_routing_window_days";
/// The referee (`crate::referee`): whether a differing twin pair asks one
/// (on unless set to off), and the bar window after a contradiction, in days.
/// Same env-suffix convention (`CAPSULES_ADJUDICATE_DIFFERING_TWINS`,
/// `CAPSULES_REFEREE_BAR_DAYS`).
pub const ADJUDICATE_DIFFERING_TWINS_KEY: &str = "adjudicate_differing_twins";
pub const REFEREE_BAR_DAYS_KEY: &str = "referee_bar_days";

/// This process's own runtime env var for `share_record_at_completion` --
/// see the module doc's "declarative only" note: no live host->plugin
/// config channel exists yet, so an operator sets this directly on the
/// node.
pub const ENV_RECORD_AT_COMPLETION: &str = "CAPSULES_SHARE_RECORD_AT_COMPLETION";

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
pub const ENV_HISTORY_SEGMENTS: &str = "CAPSULES_SHARE_HISTORY_SEGMENTS";

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

/// This process's own env var for `share_adjudications`.
pub const ENV_ADJUDICATIONS: &str = "CAPSULES_SHARE_ADJUDICATIONS";

/// Deliver a verdict to the nodes it judges: unless `share_adjudications` is
/// explicitly `off` (the default is `deliver_to_subjects`).
pub fn adjudications_delivered() -> bool {
    crate::settings::var(ENV_ADJUDICATIONS).ok().as_deref() != Some("off")
}

const CATEGORY_ID: &str = "share";
const CATEGORY_LABEL: &str = "Sharing policy";
const CATEGORY_SUMMARY: &str = "What this node shares, with whom, by default. Every default keys \
    on relationship (counterparty in the window), never proximity or latency.";

const REFEREE_CATEGORY_ID: &str = "referee";
const REFEREE_CATEGORY_LABEL: &str = "Referee";
const REFEREE_CATEGORY_SUMMARY: &str =
    "An independent check of two twins that answered the same request differently. \
    Eligibility is yes or no, from this node's own records; no figure about a peer is computed, and nothing is sent.";

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
            config_setting(WITNESS_KEY, config_array(config_url()))
                .description(
                    "Witness services to offer this node's checkpoints to, one URL each. Empty is \
                     off (the default): there is no default witness, and the plugin contacts none \
                     until you add one. The Evidence page shows what each witness holds. Takes \
                     effect when mesh-llm is restarted.",
                )
                .label("Witnesses")
                .category(CATEGORY_ID, CATEGORY_LABEL, CATEGORY_SUMMARY, 3),
        )
        .setting(
            config_setting(ADJUDICATE_DIFFERING_TWINS_KEY, config_enum(["on", "off"]))
                .default_value(&"on")
                .description(
                    "Ask a referee when two twins of a pair the host marked answered the same \
                     request at temperature 0, on the same model and weights, and differ (on, the \
                     default), or never (off). At most one call per pair, never retried. A pair \
                     with no ruling reads \"Not adjudicated\" with the reason, and never counts \
                     against either twin.",
                )
                .label("Ask a referee when twins differ")
                .category(REFEREE_CATEGORY_ID, REFEREE_CATEGORY_LABEL, REFEREE_CATEGORY_SUMMARY, 0),
        )
        .setting(
            config_setting(REFEREE_BAR_DAYS_KEY, config_integer())
                .default_value(&30)
                .description(
                    "After a referee-signed contradiction for a model, a node is not asked to \
                     referee that model for this many days (default 30), counted from when this \
                     node recorded the verdict. A later corroboration does not end it early.",
                )
                .label("Bar window, days")
                .category(REFEREE_CATEGORY_ID, REFEREE_CATEGORY_LABEL, REFEREE_CATEGORY_SUMMARY, 1),
        )
        .setting(
            config_setting(STOP_ROUTING_AFTER_KEY, config_integer())
                .description(
                    "Stop routing to a peer after this many contradictions within the window below. \
                     Off when empty or 0 (the default). Only verdicts this node asked a referee \
                     for, and whose signature it checked, count, once per referee and pair of \
                     answers; with N of 2 or more, no single referee can reach N alone. When it fires, the host blocks the peer until you undo \
                     it, exactly like Stop routing, and the sealed record names this rule and \
                     cites the verdicts. Undo it the same way. No figure about a peer is computed, and nothing is sent.",
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
        let schema = as_config_schema(share_policy_config_schema("capsules"));
        assert_eq!(schema.plugin_name, "capsules");
        let keys: Vec<&str> = schema.settings.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                RECORD_AT_COMPLETION_KEY,
                HISTORY_SEGMENTS_KEY,
                ADJUDICATIONS_KEY,
                WITNESS_KEY,
                ADJUDICATE_DIFFERING_TWINS_KEY,
                REFEREE_BAR_DAYS_KEY,
                STOP_ROUTING_AFTER_KEY,
                STOP_ROUTING_WINDOW_KEY
            ]
        );
    }

    #[test]
    fn the_referee_settings_are_the_env_names_the_referee_reads() {
        assert_eq!(
            format!(
                "CAPSULES_{}",
                ADJUDICATE_DIFFERING_TWINS_KEY.to_uppercase()
            ),
            crate::referee::request::ENV_ADJUDICATE_DIFFERING_TWINS
        );
        assert_eq!(
            format!("CAPSULES_{}", REFEREE_BAR_DAYS_KEY.to_uppercase()),
            crate::referee::bar::ENV_REFEREE_BAR_DAYS
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
        // CAPSULES_ prefix -- this module's whole "no
        // re-naming exercise later" claim rests on this correspondence.
        let expected_env_suffix = [
            (RECORD_AT_COMPLETION_KEY, "SHARE_RECORD_AT_COMPLETION"),
            (HISTORY_SEGMENTS_KEY, "SHARE_HISTORY_SEGMENTS"),
            (ADJUDICATIONS_KEY, "SHARE_ADJUDICATIONS"),
            (WITNESS_KEY, "CHECKPOINT_WITNESS_URLS"),
            (ADJUDICATE_DIFFERING_TWINS_KEY, "ADJUDICATE_DIFFERING_TWINS"),
            (REFEREE_BAR_DAYS_KEY, "REFEREE_BAR_DAYS"),
            (STOP_ROUTING_AFTER_KEY, "STOP_ROUTING_AFTER_CONTRADICTIONS"),
            (STOP_ROUTING_WINDOW_KEY, "STOP_ROUTING_WINDOW_DAYS"),
        ];
        for (key, suffix) in expected_env_suffix {
            assert_eq!(
                crate::settings::console_key(&format!("CAPSULES_{suffix}")).as_deref(),
                Some(key),
                "env suffix {suffix} must map to the console key {key}"
            );
        }
    }

    #[test]
    fn record_at_completion_and_adjudications_default_to_the_documented_on_state() {
        let schema = as_config_schema(share_policy_config_schema("capsules"));
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
        let schema = as_config_schema(share_policy_config_schema("capsules"));
        let after = schema
            .settings
            .iter()
            .find(|s| s.key == STOP_ROUTING_AFTER_KEY)
            .unwrap();
        assert_eq!(after.default_json, None, "no N by default: the rule is off");
        assert_eq!(
            format!(
                "CAPSULES_{}",
                STOP_ROUTING_AFTER_KEY.to_uppercase()
            ),
            crate::routing_rule::ENV_AFTER
        );
        assert_eq!(
            format!(
                "CAPSULES_{}",
                STOP_ROUTING_WINDOW_KEY.to_uppercase()
            ),
            crate::routing_rule::ENV_WINDOW_DAYS
        );
    }

    #[test]
    fn witness_has_no_default_url_and_is_not_required() {
        // Design note S1: "witness: off | <url> # default: off" -- off IS
        // the absence of a default, never a magic sentinel string.
        let schema = as_config_schema(share_policy_config_schema("capsules"));
        let witness = schema
            .settings
            .iter()
            .find(|s| s.key == WITNESS_KEY)
            .unwrap();
        assert_eq!(witness.default_json, None);
        assert!(!witness.required);
        // A list of URLs: several witnesses, each named by the operator.
        let value = witness.value_schema.as_ref().unwrap();
        assert_eq!(value.kind, proto::PluginConfigValueKind::Array as i32);
        assert_eq!(
            value.items.as_ref().unwrap().kind,
            proto::PluginConfigValueKind::Url as i32
        );
    }

    #[test]
    fn every_setting_is_optional_shipping_this_schema_flips_no_runtime_behavior() {
        let schema = as_config_schema(share_policy_config_schema("capsules"));
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
