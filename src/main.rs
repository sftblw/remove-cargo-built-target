use remove_cargo_built_target::cleanup::{
    CleanupTarget, CleanupTargetKind, remove_cleanup_target, scan_cleanup_targets,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use dioxus::prelude::*;

const FAVICON: Asset = asset!("/assets/favicon.ico");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

fn main() {
    dioxus::launch(App);
}

#[derive(Clone, Debug, PartialEq)]
struct CleanupTargetInfo {
    target: CleanupTarget,
    include_in_cleanup: bool,
    is_removed: bool,
    is_removing: bool,
    error_content: Option<String>,
}

#[derive(Clone, Debug)]
struct DiscoveredCleanupTarget {
    generation: u64,
    path_info: CleanupTargetInfo,
}

fn can_replace_cleanup_targets(project_paths: &BTreeMap<PathBuf, CleanupTargetInfo>) -> bool {
    !project_paths
        .values()
        .any(|target_info| target_info.is_removing)
}

fn claim_cleanup_candidates(
    project_paths: &mut BTreeMap<PathBuf, CleanupTargetInfo>,
) -> Vec<(PathBuf, CleanupTargetInfo)> {
    project_paths
        .iter_mut()
        .filter_map(|(path, target_info)| {
            (target_info.include_in_cleanup && !target_info.is_removed && !target_info.is_removing)
                .then(|| {
                    target_info.is_removing = true;
                    (path.clone(), target_info.clone())
                })
        })
        .collect()
}

fn record_cleanup_result(target_info: &mut CleanupTargetInfo, removal_result: Result<(), String>) {
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

fn record_scanned_cleanup_target(
    project_paths: &mut BTreeMap<PathBuf, CleanupTargetInfo>,
    current_generation: u64,
    discovered_target: DiscoveredCleanupTarget,
) {
    if discovered_target.generation != current_generation {
        return;
    }

    let path_info = discovered_target.path_info;
    let artifact_path = path_info.target.artifact_path.clone();
    project_paths.entry(artifact_path).or_insert(path_info);
}

enum ScanEvent {
    Found(DiscoveredCleanupTarget),
    Visiting(u64, PathBuf),
    Finished(u64, Option<String>),
}

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
            include_in_cleanup: true,
            is_removed: false,
            is_removing: false,
            error_content: None,
        }
    }

    #[test]
    fn unchecked_targets_survive_late_discovery_and_are_not_claimed() {
        let path = PathBuf::from("working/target");
        let mut excluded = target("working/target");
        excluded.include_in_cleanup = false;
        let mut paths = BTreeMap::from([(path.clone(), excluded.clone())]);
        record_scanned_cleanup_target(
            &mut paths,
            1,
            DiscoveredCleanupTarget {
                generation: 1,
                path_info: target("working/target"),
            },
        );
        assert_eq!(paths[&path], excluded);
        assert!(claim_cleanup_candidates(&mut paths).is_empty());
        paths.get_mut(&path).unwrap().include_in_cleanup = true;
        assert_eq!(claim_cleanup_candidates(&mut paths).len(), 1);
    }

    #[test]
    fn cleanup_claims_exclude_already_removed_targets() {
        let active_path = PathBuf::from("active/target");
        let removed_path = PathBuf::from("removed/target");
        let active = target("active/target");
        let mut removed = target("removed/target");
        removed.is_removed = true;
        let mut project_paths =
            BTreeMap::from([(active_path.clone(), active), (removed_path, removed)]);

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
    fn scan_replacement_waits_for_in_flight_cleanup() {
        let path = PathBuf::from("project/target");
        let target = target("project/target");
        let mut project_paths = BTreeMap::from([(path.clone(), target)]);

        assert!(can_replace_cleanup_targets(&project_paths));
        let _ = claim_cleanup_candidates(&mut project_paths);
        assert!(!can_replace_cleanup_targets(&project_paths));

        record_cleanup_result(
            project_paths.get_mut(&path).unwrap(),
            Err("permission denied".to_owned()),
        );

        assert!(can_replace_cleanup_targets(&project_paths));
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

    #[test]
    fn late_scan_results_preserve_claimed_and_removed_targets() {
        let claimed_path = PathBuf::from("claimed/target");
        let removed_path = PathBuf::from("removed/target");
        let discovered_path = PathBuf::from("discovered/target");
        let mut claimed = target("claimed/target");
        claimed.is_removing = true;
        claimed.error_content = Some("already claimed".to_owned());
        let mut removed = target("removed/target");
        removed.is_removed = true;
        let mut project_paths = BTreeMap::from([
            (claimed_path.clone(), claimed.clone()),
            (removed_path.clone(), removed.clone()),
        ]);

        record_scanned_cleanup_target(
            &mut project_paths,
            1,
            DiscoveredCleanupTarget {
                generation: 1,
                path_info: target("claimed/target"),
            },
        );
        record_scanned_cleanup_target(
            &mut project_paths,
            1,
            DiscoveredCleanupTarget {
                generation: 1,
                path_info: target("removed/target"),
            },
        );
        record_scanned_cleanup_target(
            &mut project_paths,
            1,
            DiscoveredCleanupTarget {
                generation: 1,
                path_info: target("discovered/target"),
            },
        );

        assert_eq!(project_paths[&claimed_path], claimed);
        assert_eq!(project_paths[&removed_path], removed);
        assert!(project_paths.contains_key(&discovered_path));
    }
    #[test]
    fn stale_scan_results_are_discarded_and_current_results_preserve_failed_state() {
        let failed_path = PathBuf::from("failed/target");
        let stale_path = PathBuf::from("stale/target");
        let mut failed = target("failed/target");
        failed.error_content = Some("permission denied".to_owned());
        let mut project_paths = BTreeMap::from([(failed_path.clone(), failed.clone())]);

        record_scanned_cleanup_target(
            &mut project_paths,
            2,
            DiscoveredCleanupTarget {
                generation: 1,
                path_info: target("stale/target"),
            },
        );
        record_scanned_cleanup_target(
            &mut project_paths,
            2,
            DiscoveredCleanupTarget {
                generation: 2,
                path_info: target("failed/target"),
            },
        );

        assert!(!project_paths.contains_key(&stale_path));
        assert_eq!(project_paths[&failed_path], failed);
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
    let mut scan_generation = use_signal(|| 0_u64);
    let mut search = use_signal(String::new);
    let mut scan_cancel = use_signal(|| None::<Arc<AtomicBool>>);
    let mut scan_status = use_signal(|| "Ready".to_owned());
    let scan_channel = use_hook(|| flume::bounded::<ScanEvent>(128));
    use_drop(move || {
        if let Some(cancel) = scan_cancel.peek().as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    let selected_count = use_memo(move || {
        project_paths
            .read()
            .values()
            .filter(|info| info.include_in_cleanup)
            .count()
    });
    let pending_count = use_memo(move || {
        project_paths
            .read()
            .values()
            .filter(|info| info.include_in_cleanup && !info.is_removed && !info.is_removing)
            .count()
    });
    let visible_paths = use_memo(move || {
        let query = search().trim().to_lowercase();
        project_paths
            .read()
            .iter()
            .filter(|(_, info)| {
                info.target
                    .artifact_path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&query)
            })
            .map(|(path, info)| (path.clone(), info.clone()))
            .collect::<Vec<_>>()
    });
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
        if scan_cancel.read().is_some() {
            return;
        }
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
    let scan_rx = scan_channel.1.clone();
    use_future(move || {
        let scan_rx = scan_rx.clone();
        async move {
            while let Ok(event) = scan_rx.recv_async().await {
                match event {
                    ScanEvent::Found(target) => {
                        record_scanned_cleanup_target(
                            &mut project_paths.write(),
                            scan_generation(),
                            target,
                        );
                    }
                    ScanEvent::Visiting(generation, path) if generation == scan_generation() => {
                        current_iterating_path.set(Some(path));
                    }
                    ScanEvent::Finished(generation, error) if generation == scan_generation() => {
                        let cancelled = scan_cancel
                            .read()
                            .as_ref()
                            .is_some_and(|cancel| cancel.load(Ordering::Relaxed));
                        scan_cancel.set(None);
                        scan_status.set(error.unwrap_or_else(|| {
                            if cancelled {
                                "Scan cancelled — partial results".to_owned()
                            } else {
                                "Scan complete".to_owned()
                            }
                        }));
                    }
                    _ => {}
                }
            }
        }
    });

    let mut find_paths = move || {
        if scan_cancel.read().is_some() || !can_replace_cleanup_targets(&project_paths()) {
            return;
        }

        let target_path = base_path().clone();
        let scan_tx = scan_channel.0.clone();
        let next_scan_generation = scan_generation().wrapping_add(1);
        scan_generation.set(next_scan_generation);
        clear_target_paths();
        current_iterating_path.set(None);
        scan_status.set("Scanning…".to_owned());
        let cancel = Arc::new(AtomicBool::new(false));
        scan_cancel.set(Some(cancel.clone()));
        let finished_tx = scan_tx.clone();
        let task = tokio::task::spawn_blocking(move || {
            let mut last_update = Instant::now() - Duration::from_secs(1);
            scan_cleanup_targets(
                &target_path,
                || cancel.load(Ordering::Relaxed),
                |path| {
                    if last_update.elapsed() >= Duration::from_millis(100) {
                        let _ = scan_tx.try_send(ScanEvent::Visiting(
                            next_scan_generation,
                            path.to_path_buf(),
                        ));
                        last_update = Instant::now();
                    }
                },
                |target| {
                    let mut event = ScanEvent::Found(DiscoveredCleanupTarget {
                        generation: next_scan_generation,
                        path_info: CleanupTargetInfo {
                            target,
                            include_in_cleanup: true,
                            is_removed: false,
                            is_removing: false,
                            error_content: None,
                        },
                    });
                    while !cancel.load(Ordering::Relaxed) {
                        match scan_tx.send_timeout(event, Duration::from_millis(100)) {
                            Ok(()) => break,
                            Err(flume::SendTimeoutError::Timeout(pending)) => event = pending,
                            Err(flume::SendTimeoutError::Disconnected(_)) => break,
                        }
                    }
                },
            );
        });
        spawn(async move {
            let error = task
                .await
                .err()
                .map(|error| format!("Scan failed: {error}"));
            let _ = finished_tx
                .send_async(ScanEvent::Finished(next_scan_generation, error))
                .await;
        });
    };

    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        document::Stylesheet { href: TAILWIND_CSS }
        main {
            class: "min-h-screen bg-slate-50 font-sans text-slate-800 selection:bg-teal-100",
            div {
                class: "mx-auto max-w-5xl space-y-6 px-4 py-8 sm:px-8",
                header {
                    class: "flex items-center gap-4",
                    div {
                        class: "flex size-12 shrink-0 items-center justify-center rounded-2xl bg-teal-800 text-white shadow-sm",
                        svg {
                            class: "size-6", view_box: "0 0 24 24", fill: "none",
                            stroke: "currentColor", stroke_width: "1.6", "aria-hidden": "true",
                            path { d: "M3 7h7l2 2h9v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Zm0 0V5a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v2" }
                        }
                    }
                    div {
                        h1 { class: "text-2xl font-semibold tracking-tight", "Build cleanup" }
                        p { class: "mt-1 text-sm text-slate-500", "Less build clutter. More room for what’s next." }
                    }
                }

                section {
                    class: "rounded-2xl border border-slate-200 bg-white p-5 shadow-sm",
                    "aria-label": "Scan directory",
                    div {
                        class: "flex flex-col gap-4 sm:flex-row sm:items-center",
                        div {
                            class: "min-w-0 flex-1",
                            p { class: "text-xs font-semibold uppercase tracking-wider text-slate-500", "Workspace" }
                            p {
                                class: "mt-2 break-all font-mono text-sm text-slate-700",
                                "{base_path().display()}"
                            }
                        }
                        button {
                            class: "shrink-0 rounded-lg border border-slate-300 px-4 py-2 text-sm font-medium transition hover:bg-slate-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-teal-600 disabled:cursor-not-allowed disabled:opacity-40",
                            disabled: scan_cancel.read().is_some() || !can_replace_cleanup_targets(&project_paths()),
                            onclick: move |_| {
                                let path = rfd::FileDialog::new().set_directory(base_path().clone()).pick_folder();
                                if let Some(path) = path { base_path.set(path); }
                            },
                            "Choose folder"
                        }
                    }
                    div {
                        class: "mt-5 flex flex-wrap items-center gap-3 border-t border-slate-100 pt-4",
                        button {
                            class: "rounded-lg bg-teal-800 px-4 py-2 text-sm font-semibold text-white transition hover:bg-teal-900 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-teal-600 disabled:cursor-not-allowed disabled:opacity-40",
                            disabled: scan_cancel.read().is_some() || !can_replace_cleanup_targets(&project_paths()),
                            onclick: move |_| find_paths(),
                            "Find paths"
                        }
                        if scan_cancel.read().is_some() {
                            button {
                                class: "rounded-lg border border-slate-300 px-4 py-2 text-sm font-medium hover:bg-slate-50 focus-visible:outline-2 focus-visible:outline-teal-600",
                                onclick: move |_| {
                                    if let Some(cancel) = scan_cancel.read().as_ref() {
                                        cancel.store(true, Ordering::Relaxed);
                                        scan_status.set("Cancelling…".to_owned());
                                    }
                                },
                                "Stop scan"
                            }
                        }
                        span { class: "text-xs text-slate-500", "Cargo target · Gradle build · Node modules" }
                    }
                    div {
                        class: "mt-4 flex items-center gap-2 text-xs text-slate-500",
                        span {
                            class: "size-2 shrink-0 rounded-full",
                            class: if scan_cancel.read().is_some() { "bg-teal-500 motion-safe:animate-pulse" } else { "bg-slate-300" },
                        }
                        p { role: "status", "{scan_status}" }
                    }
                    if scan_cancel.read().is_some() {
                        p {
                            class: "mt-2 truncate font-mono text-xs text-slate-400",
                            title: "{current_iterating_path().unwrap_or_default().display()}",
                            "{current_iterating_path().unwrap_or_default().display()}"
                        }
                    }
                }

                section {
                    class: "overflow-hidden rounded-2xl border border-slate-200 bg-white shadow-sm",
                    "aria-label": "Cleanup targets",
                    div {
                        class: "space-y-4 border-b border-slate-200 p-5",
                        div {
                            class: "flex flex-wrap items-center justify-between gap-3",
                            h2 { class: "text-base font-semibold", "Cleanup targets" }
                            p { class: "text-xs tabular-nums text-slate-500", "{visible_paths().len()} of {project_paths.read().len()} shown" }
                        }
                        label {
                            class: "flex items-center gap-3 rounded-xl border border-slate-300 bg-slate-50 px-3 focus-within:border-teal-600 focus-within:ring-2 focus-within:ring-teal-100",
                            svg {
                                class: "size-5 shrink-0 text-slate-400", view_box: "0 0 24 24",
                                fill: "none", stroke: "currentColor", stroke_width: "1.8", "aria-hidden": "true",
                                circle { cx: "10.5", cy: "10.5", r: "6.5" }
                                path { d: "m16 16 4.5 4.5" }
                            }
                            span { class: "sr-only", "Search project name or path" }
                            input {
                                class: "min-w-0 flex-1 bg-transparent py-3 text-sm outline-none placeholder:text-slate-400",
                                r#type: "search",
                                value: search(),
                                oninput: move |event| search.set(event.value()),
                                placeholder: "Search project name or path…",
                            }
                        }
                        div {
                            class: "flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between",
                            div {
                                p { class: "text-sm font-medium text-slate-700", "{selected_count()} selected" }
                                p { class: "mt-1 text-xs leading-relaxed text-slate-500", "Uncheck to keep. Checked items hidden by search are still included." }
                            }
                            button {
                                class: "shrink-0 rounded-lg bg-rose-600 px-4 py-2.5 text-sm font-semibold text-white transition hover:bg-rose-700 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-rose-600 disabled:cursor-not-allowed disabled:bg-slate-100 disabled:text-slate-400",
                                disabled: scan_cancel.read().is_some() || pending_count() == 0,
                                onclick: move |_| remove_paths(),
                                "Cleanup ({pending_count()})"
                            }
                        }
                        if !can_replace_cleanup_targets(&project_paths()) || !removed_paths().is_empty() {
                            div {
                                class: "space-y-2",
                                p { class: "text-xs tabular-nums text-slate-500", "{removed_paths().len()} of {selected_count()} selected artifacts removed" }
                                progress {
                                    class: "block h-1.5 w-full overflow-hidden rounded-full accent-teal-600",
                                    "aria-label": "Cleanup progress",
                                    value: removed_paths().len() as f64,
                                    max: selected_count().max(1) as f64,
                                }
                            }
                        }
                    }

                    if visible_paths().is_empty() {
                        div {
                            class: "px-6 py-14 text-center",
                            div { class: "mx-auto mb-4 flex size-12 items-center justify-center rounded-full bg-teal-50 text-xl text-teal-700", "…" }
                            if !project_paths.read().is_empty() {
                                h3 { class: "font-semibold", "No matching projects" }
                                p { class: "mt-2 text-sm text-slate-500", "Try another name or clear the search to see all targets." }
                                button {
                                    class: "mt-4 rounded-lg px-3 py-2 text-sm font-semibold text-teal-700 hover:bg-teal-50 focus-visible:outline-2 focus-visible:outline-teal-600",
                                    onclick: move |_| search.set(String::new()),
                                    "Clear search"
                                }
                            } else if scan_cancel.read().is_some() {
                                h3 { class: "font-semibold", "Looking for build artifacts…" }
                                p { class: "mt-2 text-sm text-slate-500", "Projects will appear here as they are found." }
                            } else if scan_generation() == 0 {
                                h3 { class: "font-semibold", "A little room to breathe" }
                                p { class: "mx-auto mt-2 max-w-sm text-sm leading-relaxed text-slate-500", "Choose a folder and select Find paths. Then uncheck the projects you’re working on before cleaning up." }
                            } else {
                                h3 { class: "font-semibold", "No build artifacts found" }
                                p { class: "mt-2 text-sm text-slate-500", "Try another folder or scan again." }
                            }
                        }
                    } else {
                        ul {
                            class: "divide-y divide-slate-100",
                            for (path_key, target_info) in visible_paths() {
                                li {
                                    key: "{path_key.display()}",
                                    class: "flex items-start gap-4 px-5 py-4 transition-colors",
                                    class: if target_info.is_removed { "bg-slate-50" } else if target_info.include_in_cleanup { "bg-white hover:bg-teal-50/40" } else { "bg-slate-50/70" },
                                    label {
                                        class: "flex shrink-0 cursor-pointer flex-col items-center gap-1.5 pt-1 text-xs text-slate-500",
                                        input {
                                            class: "size-4 accent-teal-700 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-teal-600 disabled:cursor-not-allowed",
                                            r#type: "checkbox",
                                            checked: target_info.include_in_cleanup,
                                            disabled: target_info.is_removed || target_info.is_removing,
                                            "aria-label": "Include {target_info.target.artifact_path.display()} in cleanup",
                                            onchange: move |event| {
                                                if let Some(info) = project_paths.write().get_mut(&path_key) {
                                                    if !info.is_removed && !info.is_removing {
                                                        info.include_in_cleanup = event.checked();
                                                    }
                                                }
                                            },
                                        }
                                        "Include"
                                    }
                                    div {
                                        class: "min-w-0 flex-1",
                                        div {
                                            class: "flex flex-wrap items-center gap-2",
                                            p {
                                                class: "break-all text-sm font-semibold text-slate-800",
                                                "{target_info.target.project_path.file_name().unwrap_or_default().to_string_lossy()}"
                                            }
                                            span {
                                                class: match target_info.target.kind {
                                                    CleanupTargetKind::Target => "rounded-md px-2 py-0.5 text-xs font-medium bg-orange-50 text-orange-800",
                                                    CleanupTargetKind::NodeModules => "rounded-md px-2 py-0.5 text-xs font-medium bg-emerald-50 text-emerald-800",
                                                    CleanupTargetKind::GradleBuild => "rounded-md px-2 py-0.5 text-xs font-medium bg-sky-50 text-sky-800",
                                                },
                                                match target_info.target.kind {
                                                    CleanupTargetKind::Target => "Cargo / target",
                                                    CleanupTargetKind::NodeModules => "Node / node_modules",
                                                    CleanupTargetKind::GradleBuild => "Gradle / build",
                                                }
                                            }
                                        }
                                        p {
                                            class: "mt-1.5 break-all font-mono text-xs leading-relaxed text-slate-500",
                                            "{target_info.target.artifact_path.display()}"
                                        }
                                        if let Some(error_content) = &target_info.error_content {
                                            p { class: "mt-2 break-words rounded-lg bg-rose-50 px-3 py-2 text-xs text-rose-700", "{error_content}" }
                                        }
                                        div {
                                            class: "mt-2 flex items-center gap-3",
                                            span {
                                                class: "text-xs font-medium",
                                                class: if target_info.is_removed { "text-teal-700" } else if target_info.error_content.is_some() { "text-rose-600" } else { "text-slate-500" },
                                                if target_info.is_removed { "Removed" }
                                                else if target_info.is_removing { "Removing…" }
                                                else if !target_info.include_in_cleanup { "Excluded · kept" }
                                                else if target_info.error_content.is_some() { "Failed · retry available" }
                                                else { "Ready to clean" }
                                            }
                                            if !target_info.is_removed {
                                                button {
                                                    class: "rounded px-1 py-0.5 text-xs font-medium text-teal-700 hover:bg-teal-50 focus-visible:outline-2 focus-visible:outline-teal-600",
                                                    onclick: move |_| {
                                                        if let Err(err) = open::that(&target_info.target.project_path) {
                                                            eprintln!("Failed to open folder: {err:?}");
                                                        }
                                                    },
                                                    "Open folder ↗"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                p { class: "text-center text-xs text-slate-400", "Only discovered build directories are removed. A new scan resets your selection." }
            }
        }
    }
}
