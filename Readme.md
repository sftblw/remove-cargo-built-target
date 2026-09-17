# Clean Cargo, Gradle, and Node build directories

that's it.

1. Select a directory and click **Find Paths** to discover Cargo `target`, Gradle `build`, and Node `node_modules` directories.
2. Each artifact needs a sibling project manifest. A Node target requires `package.json`; Gradle requires a build/settings `.gradle` or `.gradle.kts` file.
   Cargo targets require a parseable `Cargo.toml` with a top-level `package` or `workspace` table, including virtual workspaces with shared `target` directories.
3. Search by name or path, then uncheck **Include** for artifacts you want to keep.
4. Click **Cleanup (count)** after scanning finishes (or Abort completes). Only checked, pending artifacts are deleted. Removing and deleted rows cannot be unchecked.

Search filters the display only: checked hidden rows still count toward Cleanup.
**Select all visible** / **Deselect all visible** affect only the current search
results, excluding removing or removed rows. Hidden rows keep their selection.
Items discovered after a bulk selection action still default to checked.
Selections last for the current scan; starting a new scan checks all discovered artifacts again.
Results appear during scanning. Abort requests cancellation between filesystem operations;
an operating-system filesystem call that is already blocked must return before cancellation finishes.

I tried to be careful but use at your own risk

![](./readme/screenshot.png)

Styling uses Tailwind CSS 4 utility classes in `src/main.rs`. The root
`tailwind.css` is the input; Dioxus 0.7 automatically generates
`assets/tailwind.css` during `dx serve` / `dx build`. Do not edit the generated
stylesheet. It is included so checks can also run before the first DX build.

Use Dioxus for the application and stylesheet workflow:

- `dx serve` for dev,
- `dx bundle` for making installable something
