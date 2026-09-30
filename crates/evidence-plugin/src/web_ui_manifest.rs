//! The plugin's web UI projection (mesh-llm "Plugin Web UI Projection
//! Contract", `docs/plugins/README.md` in Mesh-LLM/mesh-llm): ONE page,
//! labelled `Evidence`, served from ONE bundle rooted at `bundle/` in the
//! installed package. The console mounts it at
//! `/plugins/capsule-emit-mesh/evidence`, and because it is the plugin's only
//! page the console gives it a direct navigation item rather than a
//! `Plugins` menu entry.
//!
//! The bundle is built from `web-ui/` (`pnpm build` writes
//! `bundle/register-mesh-plugin-ui.js`); the release workflow builds it and
//! packages `bundle/` next to the executable and `plugin-manifest.json`
//! (`--print-package-manifest`). The page's label is the only place the name
//! lives on the console side; host code never carries it.

use mesh_llm_plugin::{
    package_manifest_json, plugin_server_info, web_ui, web_ui_bundle, web_ui_page,
    DeclarativePluginBuilder, ManifestEntry, Plugin, PluginMetadata,
};

pub const WEB_UI_BUNDLE_ID: &str = "main";
/// Package-relative bundle root: the directory the release archive carries
/// and `web-ui/vite.config.ts` builds into.
pub const WEB_UI_BUNDLE_ROOT: &str = "bundle";
pub const EVIDENCE_PAGE_ID: &str = "evidence";
pub const EVIDENCE_PAGE_LABEL: &str = "Evidence";
/// A route slug, not a path: the console builds `/plugins/<plugin>/<route>`.
pub const EVIDENCE_PAGE_ROUTE: &str = "evidence";
/// The one file `web-ui/vite.config.ts` writes, relative to the bundle root.
pub const WEB_UI_ENTRY_SCRIPT: &str = "register-mesh-plugin-ui.js";

pub fn evidence_web_ui() -> ManifestEntry {
    web_ui()
        .bundle(web_ui_bundle(WEB_UI_BUNDLE_ID, WEB_UI_BUNDLE_ROOT))
        .page(
            web_ui_page(
                EVIDENCE_PAGE_ID,
                EVIDENCE_PAGE_LABEL,
                EVIDENCE_PAGE_ROUTE,
                WEB_UI_ENTRY_SCRIPT,
            )
            .bundle_id(WEB_UI_BUNDLE_ID),
        )
        .into()
}

/// The package manifest the installer reads from `plugin-manifest.json`: the
/// config schema and the web UI block, and nothing that needs the runtime
/// (no listener, ledger, or keys), so it can be printed at packaging time.
pub fn package_manifest_json_for(
    plugin_id: &str,
    plugin_version: &str,
    config: ManifestEntry,
) -> anyhow::Result<String> {
    let plugin = DeclarativePluginBuilder::new(PluginMetadata::new(
        plugin_id,
        plugin_version,
        plugin_server_info(plugin_id, plugin_version, plugin_id, "", None::<String>),
    ))
    .config_item(config)
    .web_ui_item(evidence_web_ui())
    .build();
    let manifest = plugin
        .manifest()
        .ok_or_else(|| anyhow::anyhow!("plugin declared no manifest"))?;
    package_manifest_json(&manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::path::Path;

    const PLUGIN_ID: &str = "capsule-emit-mesh";

    fn packaged() -> Value {
        let text = package_manifest_json_for(
            PLUGIN_ID,
            "0.1.0",
            crate::share_policy::share_policy_config_schema(PLUGIN_ID),
        )
        .expect("package manifest");
        serde_json::from_str(&text).expect("package manifest is JSON")
    }

    #[test]
    fn declares_exactly_one_evidence_page_in_one_bundle() {
        assert_eq!(
            packaged()["web_ui"],
            json!({
                "pages": [{
                    "id": "evidence",
                    "label": "Evidence",
                    "route": "evidence",
                    "bundle_id": "main",
                    "entry_script": "register-mesh-plugin-ui.js"
                }],
                "bundles": [{ "id": "main", "root_path": "bundle" }]
            })
        );
    }

    #[test]
    fn package_manifest_keeps_the_config_schema() {
        assert_eq!(packaged()["config_schema"]["plugin_name"], PLUGIN_ID);
    }

    /// The checked-in `plugin.package.json` is the expected output of
    /// `--print-package-manifest`; the release workflow packages the printed
    /// one, so a drift here means the reviewed file is not what ships.
    #[test]
    fn checked_in_package_manifest_matches_the_declaration() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin.package.json");
        let checked_in: Value = serde_json::from_str(
            &std::fs::read_to_string(&path).expect("plugin.package.json is checked in"),
        )
        .expect("plugin.package.json is JSON");
        assert_eq!(checked_in, packaged());
    }

    /// The manifest names a page id and an entry script that the bundle
    /// source must provide: the page id the bundle registers, and the one
    /// file the bundle build writes. Both live in TypeScript, so read them.
    #[test]
    fn the_bundle_source_registers_the_declared_page_and_entry_script() {
        let web_ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web-ui");
        let entry = std::fs::read_to_string(web_ui.join("src/register-mesh-plugin-ui.tsx"))
            .expect("bundle entry source");
        assert!(
            entry.contains(&format!(
                "export const EVIDENCE_PAGE_ID = '{EVIDENCE_PAGE_ID}'"
            )),
            "bundle entry must register page id `{EVIDENCE_PAGE_ID}`"
        );
        let vite = std::fs::read_to_string(web_ui.join("vite.config.ts")).expect("vite config");
        assert!(
            vite.contains(&format!("fileName: () => '{WEB_UI_ENTRY_SCRIPT}'")),
            "bundle build must write `{WEB_UI_ENTRY_SCRIPT}`"
        );
        assert!(
            vite.contains(&format!(
                "outDir: path.resolve(dirname, '../{WEB_UI_BUNDLE_ROOT}')"
            )),
            "bundle build must write into the declared bundle root `{WEB_UI_BUNDLE_ROOT}/`"
        );
    }
}
