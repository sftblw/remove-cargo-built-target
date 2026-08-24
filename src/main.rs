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
    error_content: Option<String>,
}

static FOUND_CHANNEL: LazyLock<(
    flume::Sender<CleanupTargetInfo>,
    flume::Receiver<CleanupTargetInfo>,
)> = LazyLock::new(|| flume::unbounded::<CleanupTargetInfo>());

static ITERATING_CHANNEL: LazyLock<(flume::Sender<PathBuf>, flume::Receiver<PathBuf>)> =
    LazyLock::new(|| flume::unbounded::<PathBuf>());

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

    let remove_paths = move || {
        spawn(async move {
            for (path_key, target_info) in project_paths() {
                spawn(async move {
                    let removal_result = remove_cleanup_target(&target_info.target).await;

                    let mut target_paths_lock = project_paths.write();
                    if let Some(info) = target_paths_lock.get_mut(&path_key) {
                        match removal_result {
                            Ok(()) => {
                                info.is_removed = true;
                                info.error_content = None;
                            }
                            Err(error) => {
                                info.is_removed = false;
                                info.error_content = Some(error.to_string());
                            }
                        }
                    }
                });
            }
        });
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

