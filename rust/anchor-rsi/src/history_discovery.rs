use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    path::{Component, Path, PathBuf},
};

const SOURCES: [(&str, &str, &str, &str); 3] = [
    ("codex", "CODEX_HOME", ".codex", "sessions"),
    ("claude", "CLAUDE_CONFIG_DIR", ".claude", "projects"),
    ("deepseek", "DSH_HOME", ".dsh", "sessions"),
];

pub fn discover() -> Result<Value, String> {
    let environment = ["HOME", "CODEX_HOME", "CLAUDE_CONFIG_DIR", "DSH_HOME"]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name.to_owned(), value)))
        .collect();
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    discover_with(&environment, &executable)
}

fn discover_with(
    environment: &BTreeMap<String, OsString>,
    executable: &Path,
) -> Result<Value, String> {
    let executable = executable
        .canonicalize()
        .map_err(|error| format!("cannot locate anchor-rsi executable: {error}"))?;
    if !executable.is_file() {
        return Err("anchor-rsi executable must be a regular file".into());
    }
    let executable_path = executable
        .to_str()
        .ok_or("anchor-rsi executable path must be representable in JSON")?;
    let mut collect = Map::new();
    collect.insert("rsi".into(), json!(executable_path));
    let mut sources = Vec::new();
    for (source, variable, default, child) in SOURCES {
        let configured = environment.get(variable).filter(|value| {
            !value.is_empty()
                && (source != "deepseek"
                    || value.to_str().is_none_or(|value| !value.trim().is_empty()))
        });
        let (home, via) = match configured {
            Some(value) => {
                let home = if source == "deepseek" {
                    configured_dsh_home(value, environment, &std::env::current_dir())
                } else {
                    Ok(PathBuf::from(value))
                };
                match home {
                    Ok(home) => (Some(home), variable.to_owned()),
                    Err(reason) => {
                        sources.push(json!({
                            "source": source, "status": "unavailable", "via": variable,
                            "reason": reason
                        }));
                        continue;
                    }
                }
            }
            None => (
                environment
                    .get("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(default)),
                format!("HOME/{default}"),
            ),
        };
        let Some(home) = home else {
            sources.push(json!({
                "source": source, "status": "unavailable", "via": via,
                "reason": format!("neither {variable} nor HOME is configured")
            }));
            continue;
        };
        let candidate = home.join(child).to_string_lossy().into_owned();
        let result = locate(&home, child);
        match result {
            Ok(path) => {
                collect.insert(source.into(), json!(path));
                sources.push(json!({
                    "source": source, "status": "available", "path": path,
                    "candidate": candidate, "via": via
                }));
            }
            Err(reason) => sources.push(json!({
                "source": source, "status": "unavailable", "candidate": candidate,
                "via": via, "reason": reason
            })),
        }
    }
    Ok(json!({
        "format": 1,
        "sources": sources,
        "local_inputs": {
            "collect": collect,
            "gate": {"rsi": executable_path},
            "publish": {"rsi": executable_path}
        },
        "limitations": "Discovery runs as the local host operator, not a remote web user. Only the located history subdirectories are proposed for read-only access; no credentials or tool-home roots are granted. Review local_inputs before installing it in the operator-owned Graph configuration. Missing sources are not evidence of no activity."
    }))
}

fn configured_dsh_home(
    value: &OsStr,
    environment: &BTreeMap<String, OsString>,
    working_directory: &std::io::Result<PathBuf>,
) -> Result<PathBuf, String> {
    let value = value
        .to_str()
        .ok_or("DSH_HOME cannot be represented in JSON")?;
    let path = if value == "~" || value.starts_with("~/") || value.starts_with("~\\") {
        let home = environment
            .get("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
            .ok_or("DSH_HOME tilde expansion requires an absolute HOME")?;
        if value == "~" {
            home
        } else {
            home.join(&value[2..])
        }
    } else {
        PathBuf::from(value)
    };
    let path = if path.is_absolute() {
        path
    } else {
        working_directory
            .as_ref()
            .map_err(|error| format!("DSH_HOME cannot be resolved: {error}"))?
            .join(path)
    };
    path.canonicalize()
        .map_err(|error| format!("DSH_HOME cannot be located: {error}"))
}

fn locate(home: &Path, child: &str) -> Result<PathBuf, String> {
    if !home.is_absolute()
        || home
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("tool home must be an absolute path without parent components".into());
    }
    let home = home
        .canonicalize()
        .map_err(|error| format!("tool home cannot be located: {error}"))?;
    let candidate = home.join(child);
    let path = candidate
        .canonicalize()
        .map_err(|error| format!("history directory cannot be located: {error}"))?;
    if path == home || !path.starts_with(&home) {
        return Err("history directory redirects outside the configured tool home".into());
    }
    if !path.is_dir() {
        return Err("history path is not a directory".into());
    }
    if path.to_str().is_none() {
        return Err("history path cannot be represented in the JSON authorization file".into());
    }
    fs::read_dir(&path).map_err(|error| format!("history directory is not readable: {error}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(home: &Path) -> BTreeMap<String, OsString> {
        BTreeMap::from([("HOME".into(), home.as_os_str().to_owned())])
    }

    fn executable(root: &Path) -> PathBuf {
        let executable = root.join("bin/anchor-rsi");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, "fixture").unwrap();
        executable
    }

    #[test]
    fn locates_only_existing_history_children_for_the_current_user() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("operator");
        for (_, _, default, child) in SOURCES {
            fs::create_dir_all(home.join(default).join(child)).unwrap();
            fs::write(home.join(default).join("credentials"), "not history").unwrap();
        }
        let executable = executable(root.path());
        fs::write(
            executable.parent().unwrap().join("credentials"),
            "not a tool",
        )
        .unwrap();
        let report = discover_with(&environment(&home), &executable).unwrap();
        for node in ["collect", "gate", "publish"] {
            assert_eq!(report["local_inputs"][node]["rsi"], json!(executable));
        }
        for (source, _, default, child) in SOURCES {
            assert_eq!(
                report["local_inputs"]["collect"][source],
                json!(home.join(default).join(child).canonicalize().unwrap())
            );
        }
        assert!(
            report["sources"]
                .as_array()
                .unwrap()
                .iter()
                .all(|source| { source["status"] == "available" })
        );
        assert_eq!(
            report["local_inputs"]["collect"].as_object().unwrap().len(),
            4
        );
    }

    #[test]
    fn explicit_tool_configuration_wins_and_never_falls_back_to_another_profile() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("operator");
        fs::create_dir_all(home.join(".codex/sessions")).unwrap();
        let custom = root.path().join("custom-codex");
        fs::create_dir_all(custom.join("sessions")).unwrap();
        let mut environment = environment(&home);
        environment.insert("CODEX_HOME".into(), custom.as_os_str().to_owned());
        let executable = executable(root.path());
        let report = discover_with(&environment, &executable).unwrap();
        assert_eq!(
            report["local_inputs"]["collect"]["codex"],
            json!(custom.join("sessions"))
        );
        fs::remove_dir(custom.join("sessions")).unwrap();
        let report = discover_with(&environment, &executable).unwrap();
        assert!(report["local_inputs"]["collect"].get("codex").is_none());
        assert_eq!(report["sources"][0]["status"], "unavailable");
        assert_eq!(report["sources"][0]["via"], "CODEX_HOME");
    }

    #[test]
    fn missing_or_relative_home_is_a_reported_gap_not_an_implicit_broad_grant() {
        let root = tempfile::tempdir().unwrap();
        let executable = executable(root.path());
        for home in [None, Some(Path::new("relative")), Some(root.path())] {
            let environment = home.map(environment).unwrap_or_default();
            let report = discover_with(&environment, &executable).unwrap();
            assert!(
                report["sources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|source| { source["status"] == "unavailable" })
            );
            assert_eq!(
                report["local_inputs"]["collect"].as_object().unwrap().len(),
                1
            );
        }
    }

    #[test]
    fn each_tools_explicit_directory_works_without_a_home_environment() {
        let root = tempfile::tempdir().unwrap();
        let mut environment = BTreeMap::new();
        for (source, variable, _, child) in SOURCES {
            let custom = root.path().join(format!("custom-{source}"));
            fs::create_dir_all(custom.join(child)).unwrap();
            environment.insert(variable.to_owned(), custom.into_os_string());
        }
        let report = discover_with(&environment, &executable(root.path())).unwrap();
        for (position, (source, variable, _, child)) in SOURCES.iter().enumerate() {
            assert_eq!(report["sources"][position]["via"], *variable);
            assert_eq!(report["sources"][position]["status"], "available");
            assert_eq!(
                report["local_inputs"]["collect"][source],
                json!(root.path().join(format!("custom-{source}")).join(child))
            );
        }
    }

    #[test]
    fn dsh_home_matches_tilde_relative_and_nonblank_configuration_rules() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("operator");
        let configured = home.join("custom-dsh");
        fs::create_dir_all(configured.join("sessions")).unwrap();
        let mut environment = environment(&home);
        for value in ["~/custom-dsh", "~\\custom-dsh"] {
            environment.insert("DSH_HOME".into(), value.into());
            let report = discover_with(&environment, &executable(root.path())).unwrap();
            assert_eq!(
                report["local_inputs"]["collect"]["deepseek"],
                json!(configured.join("sessions"))
            );
        }
        assert_eq!(
            configured_dsh_home(OsStr::new("./custom-dsh"), &environment, &Ok(home.clone()))
                .unwrap(),
            configured
        );
        fs::create_dir_all(home.join(" custom-dsh ")).unwrap();
        assert_eq!(
            configured_dsh_home(OsStr::new(" custom-dsh "), &environment, &Ok(home.clone()))
                .unwrap(),
            home.join(" custom-dsh ")
        );
        fs::create_dir_all(home.join(".dsh/sessions")).unwrap();
        environment.insert("DSH_HOME".into(), "  ".into());
        let report = discover_with(&environment, &executable(root.path())).unwrap();
        assert_eq!(report["sources"][2]["via"], "HOME/.dsh");
        assert_eq!(report["sources"][2]["status"], "available");
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_paths_are_reported_without_panicking_or_granting_a_lossy_path() {
        use std::os::unix::ffi::OsStringExt;

        let root = tempfile::tempdir().unwrap();
        let home = root.path().join(OsString::from_vec(vec![0xff]));
        fs::create_dir_all(home.join(".codex/sessions")).unwrap();
        let report = discover_with(&environment(&home), &executable(root.path())).unwrap();
        assert_eq!(report["sources"][0]["status"], "unavailable");
        assert!(report["local_inputs"]["collect"].get("codex").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn configured_home_can_be_linked_but_history_cannot_escape_it() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("operator");
        let target = root.path().join("configured-codex");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(target.join("sessions")).unwrap();
        std::os::unix::fs::symlink(&target, home.join(".codex")).unwrap();
        let executable = executable(root.path());
        let report = discover_with(&environment(&home), &executable).unwrap();
        assert_eq!(report["sources"][0]["status"], "available");
        fs::remove_dir(target.join("sessions")).unwrap();
        std::os::unix::fs::symlink(&home, target.join("sessions")).unwrap();
        let report = discover_with(&environment(&home), &executable).unwrap();
        assert_eq!(report["sources"][0]["status"], "unavailable");
        assert!(report["local_inputs"]["collect"].get("codex").is_none());
    }
}
