# Remove Cargo target and Node node_modules recursively

that's it.

1. Find Cargo projects with a `target` directory and Node projects with a `node_modules` directory.
2. A Node cleanup target requires a sibling `package.json`; arbitrary `node_modules` directories are ignored.
3. Remove the discovered artifact directories.

I tried to be careful but use at your own risk

![](./readme/screenshot.png)

It's ordinary Rust program so cargo run would work;<br/>
but It's also using Dioxus so you can refer to its

- `dx serve` for dev,
- `dx bundle` for making installable something