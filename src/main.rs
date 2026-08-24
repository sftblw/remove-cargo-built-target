use remove_cargo_built_target::cleanup::{
    discover_cleanup_targets, remove_cleanup_target, CleanupTarget, CleanupTargetKind,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    str::FromStr,
    sync::LazyLock,
};

use dioxus::prelude::*;

fn main() {
    dioxus::launch(App);
}

#[derive(Clone, Debug, PartialEq)]
struct CleanupTargetInfo {
    target: CleanupTarget,
    is_removed: bool,
    is_removing: bool,
    error_content: Option<String>,
}


fn claim_cleanup_candidates(
    project_paths: &mut BTreeMap<PathBuf, CleanupTargetInfo>,
) -> Vec<(PathBuf, CleanupTargetInfo)> {
    project_paths
        .iter_mut()
        .filter_map(|(path, target_info)| {
            (!target_info.is_removed && !target_info.is_removing).then(|| {
                target_info.is_removing = true;
                (path.clone(), target_info.clone())
            })
        })
        .collect()
}

fn record_cleanup_result(
    target_info: &mut CleanupTargetInfo,
    removal_result: Result<(), String>,
) {
    target_info.is_removing = false;
    match removal_result {
        Ok(()) => {
            target_info.is_removed = true;
            target_info.error_content = None;
        }
        Err(error) => {
            target_info.is_removed = false;
            target_info.error_content = Some(error);
        }
    }
}


static FOUND_CHANNEL: LazyLock<(
    flume::Sender<CleanupTargetInfo>,
    flume::Receiver<CleanupTargetInfo>,
)> = LazyLock::new(|| flume::unbounded::<CleanupTargetInfo>());

static ITERATING_CHANNEL: LazyLock<(flume::Sender<PathBuf>, flume::Receiver<PathBuf>)> =
    LazyLock::new(|| flume::unbounded::<PathBuf>());

#[cfg(test)]
mod tests {
    use super::*;

    fn target(path: &str) -> CleanupTargetInfo {
        let artifact_path = PathBuf::from(path);
        CleanupTargetInfo {
            target: CleanupTarget {
                kind: CleanupTargetKind::Target,
                project_path: artifact_path.parent().unwrap().to_path_buf(),
                artifact_path,
            },
            is_removed: false,
            is_removing: false,
            error_content: None,
        }
    }

    #[test]
    fn cleanup_claims_exclude_already_removed_targets() {
        let active_path = PathBuf::from("active/target");
        let removed_path = PathBuf::from("removed/target");
        let active = target("active/target");
        let mut removed = target("removed/target");
        removed.is_removed = true;
        let mut project_paths = BTreeMap::from([
            (active_path.clone(), active),
            (removed_path, removed),
        ]);

        let claimed = claim_cleanup_candidates(&mut project_paths);

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].0, active_path);
        assert!(claimed[0].1.is_removing);
    }

    #[test]
    fn cleanup_claims_prevent_concurrent_scheduling() {
        let path = PathBuf::from("project/target");
        let target = target("project/target");
        let mut project_paths = BTreeMap::from([(path.clone(), target)]);

        let claimed = claim_cleanup_candidates(&mut project_paths);

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].0, path);
        assert!(claimed[0].1.is_removing);
        assert!(project_paths[&path].is_removing);
        assert!(claim_cleanup_candidates(&mut project_paths).is_empty());
    }

    #[test]
    fn failed_cleanup_releases_its_claim_for_retry() {
        let path = PathBuf::from("project/target");
        let target = target("project/target");
        let mut project_paths = BTreeMap::from([(path.clone(), target)]);

        let _ = claim_cleanup_candidates(&mut project_paths);
        record_cleanup_result(
            project_paths.get_mut(&path).unwrap(),
            Err("permission denied".to_owned()),
        );
        assert!(!project_paths[&path].is_removing);

        let retried = claim_cleanup_candidates(&mut project_paths);

        assert_eq!(retried.len(), 1);
        assert_eq!(retried[0].0, path.clone());
        assert!(retried[0].1.is_removing);
        assert_eq!(
            project_paths[&path].error_content.as_deref(),
            Some("permission denied")
        );
    }
}

#[component]
fn App() -> Element {
    let mut base_path = use_signal(|| PathBuf::from_str("./").unwrap());
    let mut update_path = move |path_buf: PathBuf| {
        let path = path_buf.canonicalize()?;
        base_path.set(path);

        Result::<(), anyhow::Error>::Ok(())
    };

    let mut project_paths = use_signal(BTreeMap::<PathBuf, CleanupTargetInfo>::new);
    let removed_paths = use_memo(move || {
        project_paths()
            .iter()
            .filter(|(_, info)| info.is_removed)
            .map(|(path, info)| (path.clone(), info.clone()))
            .collect::<BTreeMap<PathBuf, CleanupTargetInfo>>()
    });

    
    use_effect(move || {
        let _ = update_path(PathBuf::from_str("./").unwrap());
    });

    let mut clear_target_paths = move || {
        let new_paths = BTreeMap::new();
        project_paths.set(new_paths);
    };

    let mut remove_paths = move || {
        let cleanup_targets = {
            let mut target_paths_lock = project_paths.write();
            claim_cleanup_candidates(&mut target_paths_lock)
        };

        for (path_key, target_info) in cleanup_targets {
            spawn(async move {
                let removal_result = remove_cleanup_target(&target_info.target)
                    .await
                    .map_err(|error| error.to_string());

                let mut target_paths_lock = project_paths.write();
                if let Some(info) = target_paths_lock.get_mut(&path_key) {
                    record_cleanup_result(info, removal_result);
                }
            });
        }
    };

    let mut current_iterating_path = use_signal(|| Option::<PathBuf>::None);
    let mut cur_iter_task = use_signal(|| Option::<tokio::task::JoinHandle<()>>::None);
    
    use_future(move || {
        async move {
            let found_rx = FOUND_CHANNEL.1.clone();
            while let Ok(path_info) = found_rx.recv_async().await {
                let mut project_paths_lock = project_paths.write();
                project_paths_lock.insert(path_info.target.artifact_path.clone(), path_info);
            }
        }
    });

    use_future(move || {
        async move {
            let iterating_rx = ITERATING_CHANNEL.1.clone();
            while let Ok(path) = iterating_rx.recv_async().await {
                current_iterating_path.set(Some(path));
            }
        }
    });

    let mut find_paths = move || {
        let target_path = base_path().clone();
        let found_tx = FOUND_CHANNEL.0.clone();
        let iterating_tx = ITERATING_CHANNEL.0.clone();
        clear_target_paths();

        let cur_iter_task_created = tokio::task::spawn_blocking(move || {
            let targets = discover_cleanup_targets(target_path.as_path());

            for target in targets {
                let Ok(()) = iterating_tx.send(target.project_path.clone()) else {
                    eprintln!("Failed to send iterating path");
                    break;
                };
                let path_info = CleanupTargetInfo {
                    target,
                    is_removed: false,
                    is_removing: false,
                    error_content: None,
                };
                let Ok(()) = found_tx.send(path_info) else {
                    eprintln!("Failed to send found path");
                    break;
                };
            }
        });

        cur_iter_task.set(Some(cur_iter_task_created));
    };

    rsx! {
        style {
            r#"* {{ margin: 0; padding: 0; }}"#
        }
        div {
            style: " position: relative;
            display: flex; flex-direction: column; align-items: center; justify-content: flex-start;",
            h1 { "Remove Cargo target and Node node_modules recursively" }
            div {
                style: "display: flex; flex-direction: row; align-items: center; justify-content: flex-start; flex-grow: 1 flex-shrink: 0; width: 100%;",
                p {
                    style: "text-align: center; border: 1px solid gray; padding: 10px; border-radius: 5px; flex-grow: 1; flex-shrink: 0;",
                    "{ base_path().display() }"
                }
                button {
                    style: "margin: 10px; padding: 10px; border-radius: 5px; background-color: #007bff; color: white;",

                    onclick: move |_| {
                        let path = rfd::FileDialog::new()
                            .set_directory(base_path().clone())
                            .pick_folder();

                        if let Some(path) = path {
                            base_path.set(path);
                        }
                    },
                    "Select Directory",
                }
            }

            div {
                style: "padding: 1em; text-align: center; position: relative;
                display: flex; flex-direction: column; align-items: center; justify-content: center;",
                p {
                    "👓 looking up 👓"
                }
                p {
                    style: "padding: 0.5em; font-size: 0.9em; color: gray; height: 2em; overflow: hidden; text-overflow: ellipsis;",
                    "{ current_iterating_path().unwrap_or_default().display() }"
                }
            }

            progress {
                style: "width: 100%;",
                value: removed_paths().len() as f64,
                max: project_paths().len() as f64
            }

            div {
                style: "display: flex; flex-direction: row; align-items: center; justify-content: center; flex-grow: 1 flex-shrink: 0; width: 100%;",

                button {
                    style: "margin: 10px; padding: 10px; border-radius: 5px; background-color: #007bff; color: white;",
                    onclick: move |_| {
                        find_paths();
                    },
                    "Find Paths",
                }

                button {
                    style: "margin: 10px; padding: 10px; border-radius: 5px; background-color: #007bff; color: white;",
                    
                    disabled: cur_iter_task.read().is_none(),
                    onclick: move |_| {
                        if let Some(task) = cur_iter_task.take() {
                            task.abort();
                            cur_iter_task.set(None);
                        }
                    },
                    "Abort"
                }

                button {
                    style: "margin: 10px; padding: 10px; border-radius: 5px; background-color: #007bff; color: white;",
                    onclick: move |_| {
                        remove_paths();
                    },
                    "Cleanup",
                }
            }

            ul {
                style: "list-style-type: none; padding: 0; margin: 0; width: 100%;
                display: flex; flex-direction: column; align-items: stretch; justify-content: flex-start; gap: 1em; flex-grow: 1; flex-shrink: 0;",
                for (_path_key, target_info) in project_paths() {
                    li {
                        style: "display: flex; flex-direction: row; align-items: center; justify-content: flex-start; gap: 0.5em;
                        border: 1px solid gray; padding: 10px; border-radius: 5px; flex-grow: 1; flex-shrink: 0;

                        ",
                        div {
                            style: "display: flex; flex-direction: column; flex-grow: 1; flex-shrink: 0; gap: 0.5em;",
                            p {
                                match target_info.target.kind {
                                    CleanupTargetKind::Target => "Cargo target",
                                    CleanupTargetKind::NodeModules => "Node node_modules",
                                }
                            }
                            p {
                                "{ target_info.target.project_path.file_name().unwrap_or_default().to_string_lossy() }"
                            }
                            p {
                                style: "flex-grow: 1; flex-shrink: 0; width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;",
                                "{ target_info.target.project_path.display() }",
                            }
                            if let Some(error_content) = &target_info.error_content {
                                p {
                                    style: "color: red; font-weight: bold;",
                                    "{ error_content }"
                                }
                            }
                        }

                        p {
                            style: "flex-grow: 0;
                            border: 1px solid gray; padding: 10px; border-radius: 5px;
                            ",
                            if target_info.is_removed {
                                "🗑️ DELETED"
                            } else {
                                "👁️ EXISTS"
                            }
                        }
                        if !target_info.is_removed {
                            button {
                                style: "margin: 10px; padding: 10px; border-radius: 5px; background-color: #007bff; color: white;",
                                onclick: move |_| {
                                    let path = target_info.target.project_path.clone();

                                    // open project path in platforms' file explorer
                                    if let Err(err) = open::that(&path) {
                                        eprintln!("Failed to open folder: {:?}", err);
                                    }
                                },
                                "📂 OPEN",
                            }
                        }
                    }
                }
            }
        }

    }
}

