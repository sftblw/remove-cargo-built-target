# Remove Cargo.toml project /target recursively

that's it.

1. Find out `Cargo.toml` and its `target` dir
2. remove the `target` dirs
3. STORAGE! MOAR SPACE!

I tried to be careful but use at your own risk

![](./readme/screenshot.png)

It's ordinary Rust program so cargo run would work;<br/>
but It's also using Dioxus so you can refer to its

- `dx serve` for dev,
- `dx bundle` for making installable something