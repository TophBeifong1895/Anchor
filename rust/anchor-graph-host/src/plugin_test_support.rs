use crate::FilePluginCatalog;
use std::fs;

/// A Plugin that declares Python dependencies and launches its server on the
/// interpreter of the view that declaration maps to.
pub(crate) fn catalog_with_dependency_plugin() -> (tempfile::TempDir, FilePluginCatalog) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/probe");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("plugin.json"),
        r#"{"name":"Probe","dependencies":{"python":["idna==3.6"]},"mcpServers":{"probe":{"command":"${ANCHOR_PLUGIN_VIEW}/bin/python","args":["server.py"]}}}"#,
    )
    .unwrap();
    let catalog = FilePluginCatalog::new(tmp.path());
    (tmp, catalog)
}

pub(crate) fn catalog_with_plugin() -> (tempfile::TempDir, FilePluginCatalog) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/demo");
    fs::create_dir_all(dir.join("skills/example")).unwrap();
    fs::write(dir.join("plugin.json"), r#"{"name":"Demo","mcpServers":{"inline":{"command":"run","env":{"TOKEN":"top-secret"}},"disabled":{"enabled":false,"url":"https://example.test/mcp"}}}"#).unwrap();
    fs::write(
        dir.join(".mcp.json"),
        r#"{"mcpServers":{"config":{"url":"https://example.test/mcp","headers":{"Authorization":"secret-value"}}}}"#,
    )
    .unwrap();
    fs::write(dir.join("skills/example/SKILL.md"), "hello").unwrap();
    let catalog = FilePluginCatalog::new(tmp.path());
    (tmp, catalog)
}
