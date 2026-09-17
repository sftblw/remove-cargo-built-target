# Dioxus 0.7.10 and Node Modules Cleanup Design

> Historical design, amended September 17: selection and name/path filtering are
> supported, alongside the existing local Gradle build cleanup. See `AGENTS.md`.

## Goal

Upgrade the desktop application from Dioxus 0.6.3 to Dioxus 0.7.10 and allow it to discover and remove both Rust `target` directories and Node `node_modules` directories beneath a user-selected base directory.

## Scope

- Pin the direct `dioxus` dependency to `0.7.10` with the existing `desktop` feature and regenerate `Cargo.lock`.
- Discover a cleanup target when its parent directory contains:
  - a Cargo `Cargo.toml` with a `[package]` section and a direct `target` child directory; or
  - a Node `package.json` and a direct `node_modules` child directory.
- Represent each target directory as an independent list entry. A directory containing both manifests may contribute both a `target` and a `node_modules` entry.
- Preserve the existing single global Cleanup action. It deletes selected pending targets after scanning finishes or is cancelled.
- Update the user-visible title, status text, and README so the application describes both target kinds.

## Non-goals

- No per-item delete control, confirmation dialog, package-manager integration, or manifest parsing beyond the current Cargo `[package]` check. Per-item selection is supported.
- No deletion of an arbitrary directory merely because it is named `node_modules`; its parent must contain `package.json`.
- No recursion into `target` or `node_modules` contents.

## Data Model

Replace the Cargo-specific result record with a cleanup-target record containing:

- cleanup target kind: `Target` or `NodeModules`;
- project root path, used for display and the existing Open action;
- exact artifact path, used as the only deletion argument;
- removal state; and
- optional deletion error text.

The artifact path is the map identity. It is unique even when one project root supplies two artifacts. A successful deletion sets the removal state only after `remove_dir_all` succeeds. A missing artifact or an I/O failure leaves the entry as not removed and records the error.

## Discovery and Deletion

The scanner continues to walk only directories beneath the selected base path. It prunes hidden directories and existing non-project traversal exclusions, including `target` and `node_modules`, before descending. At each eligible directory it independently evaluates the Cargo and Node rules, emits one entry for each matching artifact, and never uses a discovered artifact as a traversal root.

The cleanup action iterates the discovered target records and asynchronously invokes `tokio::fs::remove_dir_all` for the record's exact artifact path. Each result updates only the record keyed by that path. This keeps a Cargo cleanup from ever deleting a sibling Node dependency directory, and vice versa.

## User Interface

The list shows the target kind and the project root. Its status distinguishes an existing artifact, an excluded artifact, an in-progress deletion, a successful deletion, and a deletion error. Open continues to open the project root. Include checkboxes default to checked and are disabled during and after deletion. Cleanup acts on selected pending entries, including those hidden by name/path search. Progress counts selected entries only.

## Dioxus Compatibility

Dioxus 0.7.10 is the stable 0.7 release selected for this project; 0.8.0-alpha.1 is pre-release. The application uses desktop rendering and no forms, assets, server functions, custom renderers, or prelude members removed in 0.7. The upgrade therefore changes the manifest and resolved dependency graph first, then addresses only compiler-reported API incompatibilities.

## Verification

- Unit-test discovery from temporary filesystem layouts: Cargo only, Node only, a mixed root, and a `node_modules` directory without `package.json`.
- Unit-test deletion-result state: success, a path that disappeared before cleanup, and an I/O error where the platform permits deterministic construction.
- Run `cargo check` after the dependency upgrade.
- Launch the desktop application and exercise scanning and cleanup against a disposable directory tree containing both artifact kinds.
