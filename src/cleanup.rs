use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
use walkdir::{DirEntry, WalkDir};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CleanupTargetKind {
    Target,
    NodeModules,
    GradleBuild,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CleanupTarget {
    pub kind: CleanupTargetKind,
    pub project_path: PathBuf,
    pub artifact_path: PathBuf,
}

pub fn discover_cleanup_targets(base_path: &Path) -> Vec<CleanupTarget> {
    let mut targets = Vec::new();
    scan_cleanup_targets(base_path, || false, |_| {}, |target| targets.push(target));
    targets.sort_by(|left, right| left.artifact_path.cmp(&right.artifact_path));
    targets
}

/// Callbacks run on the scanning thread. Check cancellation before advancing the walker.
pub fn scan_cleanup_targets(
    base_path: &Path,
    cancelled: impl Fn() -> bool,
    mut visiting: impl FnMut(&Path),
    mut found: impl FnMut(CleanupTarget),
) {
    let walker = WalkDir::new(base_path)
        .into_iter()
        .filter_entry(should_descend);

    let mut walker = walker;
    while !cancelled() {
        let Some(entry) = walker.next() else { break };
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("Scan: {error}");
                continue;
            }
        };
        if !entry.file_type().is_dir() {
            continue;
        }

        let project_path = entry.into_path();
        visiting(&project_path);
        let target_path = project_path.join("target");
        if target_path.is_dir() && cargo_manifest_declares_project(&project_path) {
            found(CleanupTarget {
                kind: CleanupTargetKind::Target,
                project_path: project_path.clone(),
                artifact_path: target_path,
            });
        }

        let build_path = project_path.join("build");
        if build_path.is_dir()
            && [
                "build.gradle",
                "build.gradle.kts",
                "settings.gradle",
                "settings.gradle.kts",
            ]
            .iter()
            .any(|marker| project_path.join(marker).is_file())
        {
            found(CleanupTarget {
                kind: CleanupTargetKind::GradleBuild,
                project_path: project_path.clone(),
                artifact_path: build_path,
            });
        }

        let node_modules_path = project_path.join("node_modules");
        if project_path.join("package.json").is_file() && node_modules_path.is_dir() {
            found(CleanupTarget {
                kind: CleanupTargetKind::NodeModules,
                project_path,
                artifact_path: node_modules_path,
            });
        }
    }
}

pub async fn remove_cleanup_target(target: &CleanupTarget) -> std::io::Result<()> {
    tokio::fs::remove_dir_all(&target.artifact_path).await
}

fn cargo_manifest_declares_project(project_path: &Path) -> bool {
    let manifest_path = project_path.join("Cargo.toml");
    // Do not open pipes, devices, or symlinks while looking for a manifest.
    if !std::fs::symlink_metadata(&manifest_path)
        .is_ok_and(|metadata| metadata.file_type().is_file())
    {
        return false;
    }
    let Ok(mut file) = File::open(manifest_path) else {
        return false;
    };
    let mut contents = String::new();
    if file.read_to_string(&mut contents).is_err() {
        return false;
    }
    let Ok(manifest) = contents.parse::<toml::Value>() else {
        return false;
    };
    // Virtual workspaces own the shared target directory without a package table.
    // Parse TOML so comments and string values cannot masquerade as project tables.
    ["package", "workspace"]
        .iter()
        .any(|key| manifest.get(*key).is_some_and(toml::Value::is_table))
}

fn should_descend(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }

    let is_selected_root = entry.depth() == 0;

    let Some(file_name) = entry.path().file_name() else {
        return true;
    };

    if !is_selected_root
        && file_name
            .to_str()
            .is_some_and(|file_name| file_name.starts_with('.'))
    {
        return false;
    }

    const EXCLUDED_DIRS: &[&str] = &[
        "target",
        "debug",
        "release",
        "build",
        ".git",
        "node_modules",
        "dist",
        "out",
        "tmp",
        "temp",
        "cache",
        "log",
        "logs",
        "vendor",
        "assets",
        "Assets",
        "public",
        "static",
        "bin",
        "lib",
        "include",
        "libexec",
        "share",
        "local",
        "etc",
        "var",
        "run",
        "srv",
        "opt",
        "src",
        "test",
        "tests",
        "examples",
        "docs",
        "obj",
        "res",
        "Library",
    ];

    !EXCLUDED_DIRS
        .iter()
        .any(|excluded_dir| file_name == std::ffi::OsStr::new(excluded_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::{Path, PathBuf},
    };
    use tempfile::tempdir;

    fn directory(root: &Path, relative: &str) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn discovers_workspace_and_package_manifests_beyond_the_old_prefix() {
        let temp = tempdir().unwrap();
        for (name, manifest) in [
            ("virtual-workspace", "[workspace]\nmembers = []\n"),
            (
                "workspace-package",
                "[workspace]\nmembers = []\n[package]\nname = 'root'\n",
            ),
            (
                "commented-package",
                "# A long introductory comment before the package table\n\n[package]\nname = 'example'\n",
            ),
            (
                "spaced-workspace",
                "# Workspace configuration\n[ workspace ] # shared artifacts\nmembers = []\n",
            ),
        ] {
            let project = directory(temp.path(), name);
            fs::write(project.join("Cargo.toml"), manifest).unwrap();
            directory(&project, "target");
            let targets = discover_cleanup_targets(&project);
            assert_eq!(targets.len(), 1, "{name}");
            assert_eq!(targets[0].artifact_path, project.join("target"));
            assert_eq!(targets[0].kind, CleanupTargetKind::Target);
        }
    }

    #[test]
    fn does_not_treat_comments_strings_or_invalid_toml_as_project_tables() {
        let temp = tempdir().unwrap();
        for (name, manifest) in [
            ("comment", "# [package]\n# [workspace]\n"),
            ("string", "description = '''\n[workspace]\n[package]\n'''\n"),
            ("scalar", "workspace = 'not a table'\n"),
            ("nested", "[metadata.package]\nname = 'not a project'\n"),
            ("invalid", "[workspace]\nmembers = [\n"),
        ] {
            let project = directory(temp.path(), name);
            fs::write(project.join("Cargo.toml"), manifest).unwrap();
            directory(&project, "target");
            assert!(discover_cleanup_targets(&project).is_empty(), "{name}");
        }
        directory(temp.path(), "missing-manifest/target");
        assert!(discover_cleanup_targets(temp.path()).is_empty());
    }

    #[test]
    fn streaming_scan_can_cancel_after_a_discovery() {
        let temp = tempdir().unwrap();
        for name in ["first", "second"] {
            let project = directory(temp.path(), name);
            fs::write(project.join("package.json"), "{}").unwrap();
            directory(&project, "node_modules");
        }
        let cancelled = std::cell::Cell::new(false);
        let mut targets = Vec::new();
        scan_cleanup_targets(
            temp.path(),
            || cancelled.get(),
            |_| {},
            |target| {
                targets.push(target);
                cancelled.set(true);
            },
        );
        assert_eq!(targets.len(), 1);
    }

    #[test]
    fn gradle_requires_a_marker_and_does_not_scan_build_contents() {
        let temp = tempdir().unwrap();
        let project = directory(temp.path(), "gradle");
        fs::write(project.join("build.gradle.kts"), "").unwrap();
        let nested = directory(&project, "build/nested");
        fs::write(nested.join("package.json"), "{}").unwrap();
        directory(&nested, "node_modules");
        directory(temp.path(), "orphan/build");
        let targets = discover_cleanup_targets(temp.path());
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, CleanupTargetKind::GradleBuild);
        assert_eq!(targets[0].artifact_path, project.join("build"));
    }

    #[test]
    fn discovers_only_manifest_backed_artifacts() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let cargo = directory(root, "cargo-project");
        fs::write(
            cargo.join("Cargo.toml"),
            "[package]\nname = \"cargo-project\"\n",
        )
        .unwrap();
        directory(&cargo, "target");

        let node = directory(root, "node-project");
        fs::write(node.join("package.json"), "{}\n").unwrap();
        directory(&node, "node_modules");

        let hybrid = directory(root, "hybrid-project");
        fs::write(
            hybrid.join("Cargo.toml"),
            "[package]\nname = \"hybrid-project\"\n",
        )
        .unwrap();
        fs::write(hybrid.join("package.json"), "{}\n").unwrap();
        directory(&hybrid, "target");
        directory(&hybrid, "node_modules");

        let orphan = directory(root, "orphan");
        directory(&orphan, "node_modules");

        let targets = discover_cleanup_targets(root);
        assert_eq!(targets.len(), 4);
        assert!(targets.iter().any(|target| {
            target.kind == CleanupTargetKind::Target && target.artifact_path == cargo.join("target")
        }));
        assert!(targets.iter().any(|target| {
            target.kind == CleanupTargetKind::NodeModules
                && target.artifact_path == node.join("node_modules")
        }));
        assert!(targets.iter().any(|target| {
            target.kind == CleanupTargetKind::Target
                && target.artifact_path == hybrid.join("target")
        }));
        assert!(targets.iter().any(|target| {
            target.kind == CleanupTargetKind::NodeModules
                && target.artifact_path == hybrid.join("node_modules")
        }));
    }

    #[test]
    fn does_not_traverse_an_excluded_selected_root() {
        let temp = tempdir().unwrap();

        for excluded_root_name in ["target", "node_modules"] {
            let excluded_root = directory(temp.path(), excluded_root_name);
            let nested_project = directory(&excluded_root, "nested-project");
            fs::write(
                nested_project.join("Cargo.toml"),
                "[package]\nname = \"nested-project\"\n",
            )
            .unwrap();
            directory(&nested_project, "target");

            assert!(
                discover_cleanup_targets(&excluded_root).is_empty(),
                "must not traverse selected {excluded_root_name} directory"
            );
        }
    }

    #[tokio::test]
    async fn removal_reports_missing_artifact() {
        let temp = tempdir().unwrap();
        let target = CleanupTarget {
            kind: CleanupTargetKind::Target,
            project_path: temp.path().to_path_buf(),
            artifact_path: temp.path().join("missing-target"),
        };

        let error = remove_cleanup_target(&target).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn removal_deletes_exact_artifact() {
        let temp = tempdir().unwrap();
        let project = directory(temp.path(), "project");
        let artifact = directory(&project, "node_modules");
        fs::write(project.join("package.json"), "{}\n").unwrap();
        fs::write(artifact.join("package.json"), "must not survive\n").unwrap();

        let target = CleanupTarget {
            kind: CleanupTargetKind::NodeModules,
            project_path: project.clone(),
            artifact_path: artifact.clone(),
        };
        remove_cleanup_target(&target).await.unwrap();

        assert!(!artifact.exists());
        assert!(project.join("package.json").exists());
    }
}
