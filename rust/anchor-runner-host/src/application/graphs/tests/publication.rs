use super::*;

struct LibraryRoot(Option<std::ffi::OsString>);

impl LibraryRoot {
    fn set(path: &Path) -> Self {
        let previous = std::env::var_os("ANCHOR_RUNNER_LIBRARY_ROOT");
        unsafe { std::env::set_var("ANCHOR_RUNNER_LIBRARY_ROOT", path) };
        Self(previous)
    }
}

impl Drop for LibraryRoot {
    fn drop(&mut self) {
        unsafe {
            match &self.0 {
                Some(previous) => std::env::set_var("ANCHOR_RUNNER_LIBRARY_ROOT", previous),
                None => std::env::remove_var("ANCHOR_RUNNER_LIBRARY_ROOT"),
            }
        }
    }
}

/// Saving the next Graph must not rewrite live authorization or remove the
/// Library, including the supported layout where Library and bundle share a root.
#[tokio::test]
async fn publishing_network_and_plugin_changes_preserves_live_runs_and_library_bytes() {
    let _environment = crate::PROCESS_ENV_LOCK.lock().await;
    for single_root in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let catalog = root.path().join("catalog");
        let bundle_path = if single_root {
            catalog.clone()
        } else {
            catalog.join("assistant")
        };
        let _library = LibraryRoot::set(&catalog);
        for (id, network) in [("offline", false), ("network", true)] {
            let plugin = catalog.join("plugins").join(id);
            std::fs::create_dir_all(&plugin).unwrap();
            std::fs::write(
                plugin.join("plugin.json"),
                json!({"name":id,"network":network}).to_string(),
            )
            .unwrap();
        }
        let original = json!({
            "objective":"frozen assistant","entry":"work",
            "agents":{"worker":{"model":"fixture","network":false}},
            "nodes":[{"id":"work","agent":"worker","plugins":["offline"]}],
            "edges":[]
        });
        write_graph_bundle_from_catalog(&bundle_path, &original, &catalog).unwrap();
        let application = RunApplication::new(root.path().join("state"), catalog.clone())
            .with_configured_graph("assistant".into(), bundle_path.clone());
        let (_, bundle) = application.load_graph("assistant").unwrap();
        let mut record =
            GraphRunRecord::create_with_id(bundle.snapshot, json!({}), "live-assistant").unwrap();
        record.status = RunStatus::Running;
        record.plugin_bindings = referenced_plugin_bindings(&catalog, &["offline".into()]).unwrap();
        record.plugin_bindings_initialized = true;
        application.store().save(&record).unwrap();
        metadata::save(
            &application.data_root,
            &RunMetadata::new(
                record.run_id.clone(),
                "assistant".into(),
                record.graph_digest.clone(),
                &bundle_path,
            )
            .unwrap(),
        )
        .unwrap();
        application.active.lock().await.insert(
            record.run_id.clone(),
            crate::application::ActiveRun::new(
                RunApplication::graph_identity(&bundle_path).unwrap(),
                crate::application::new_control(),
            ),
        );
        let frozen_path = application.data_root.join("runs/live-assistant.json");
        let frozen_bytes = std::fs::read(&frozen_path).unwrap();
        let plugin_bytes = files(&catalog.join("plugins"));
        let mut next = original;
        next["agents"]["worker"]["network"] = json!(true);
        next["nodes"][0]["plugins"] = json!(["offline", "network"]);
        application
            .update_graph("assistant", next.clone())
            .await
            .unwrap();

        assert_eq!(std::fs::read(&frozen_path).unwrap(), frozen_bytes);
        assert_eq!(files(&catalog.join("plugins")), plugin_bytes);
        assert_eq!(
            application.read_graph("assistant").unwrap()["definition"],
            next
        );
        let (_, published) = application.load_graph("assistant").unwrap();
        assert!(published.snapshot.agents["worker"].network);
        assert_eq!(published.snapshot.nodes[0].plugins.len(), 2);
        assert!(!record.snapshot.agents["worker"].network);
        assert_eq!(record.plugin_bindings.len(), 1);
        assert!(application.active_runs(None).await.contains(&record.run_id));
    }
}
