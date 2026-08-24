use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};
use walkdir::{DirEntry, WalkDir};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CleanupTargetKind {
    Target,
    NodeModules,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CleanupTarget {
    pub kind: CleanupTargetKind,
    pub project_path: PathBuf,
    pub artifact_path: PathBuf,
}

pub fn discover_cleanup_targets(base_path: &Path) -> Vec<CleanupTarget> {
    let walker = WalkDir::new(base_path)
        .into_iter()
        .filter_entry(should_descend);
    let mut targets = Vec::new();

    for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_dir() {
            continue;
        }

        let project_path = entry.into_path();
        let target_path = project_path.join("target");
        if cargo_manifest_declares_package(&project_path) && target_path.is_dir() {
            targets.push(CleanupTarget {
                kind: CleanupTargetKind::Target,
                project_path: project_path.clone(),
                artifact_path: target_path,
            });
        }

        let node_modules_path = project_path.join("node_modules");
        if project_path.join("package.json").is_file() && node_modules_path.is_dir() {
            targets.push(CleanupTarget {
                kind: CleanupTargetKind::NodeModules,
                project_path,
                artifact_path: node_modules_path,
            });
        }
    }

    targets.sort_by(|left, right| left.artifact_path.cmp(&right.artifact_path));
    targets
}

pub async fn remove_cleanup_target(target: &CleanupTarget) -> std::io::Result<()> {
    tokio::fs::remove_dir_all(&target.artifact_path).await
}

fn cargo_manifest_declares_package(project_path: &Path) -> bool {
    let Ok(file) = File::open(project_path.join("Cargo.toml")) else {
        return false;
    };
    let mut manifest_prefix = String::new();
    let maximum_manifest_prefix_len = "[package]".len() + 10;

    BufReader::new(file)
        .take(maximum_manifest_prefix_len as u64)
        .read_to_string(&mut manifest_prefix)
        .is_ok()
        && manifest_prefix.contains("[package]")
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
            target.kind == CleanupTargetKind::Target
                && target.artifact_path == cargo.join("target")
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
