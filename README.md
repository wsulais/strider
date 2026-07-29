<!--
SPDX-FileCopyrightText: 2026 Strider contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Strider

**Out-of-core point cloud processing** — a Rust crate ecosystem and a Qt6 desktop
workbench for point clouds that do not fit in memory. It reads a cloud from a local file
or over HTTP, renders it, classifies it, edits it and writes results — without ever
materialising the whole dataset.

Strider is two things deliberately: permissively licensed libraries meant to be adopted
by other tools, and a copyleft desktop application. The boundary between them is the
crate.

## Status

Pre-release. The design is specified and documented; implementation is in progress.

## Repository layout

| Crate | What it is | License |
| --- | --- | --- |
| `strider-core` | Spatial access and the core execution model | Apache-2.0 OR MIT |
| `strider-io` (+ `strider-io-copc`) | Point cloud format adapters | Apache-2.0 OR MIT |
| `strider-algo` | Processing operators | Apache-2.0 OR MIT |
| `strider-view` (+ `strider-view-wgpu`) | The renderer and its GPU backend | Apache-2.0 OR MIT |
| `strider-doc` | The document model and its edits | Apache-2.0 OR MIT |
| `strider-editor-qt` | The desktop workbench on Qt6 | AGPL-3.0-or-later |

The design lives in [`CONTEXT.md`](CONTEXT.md) (the shared vocabulary) and under
[`docs/`](docs/) (RFC specifications and architecture decisions). `gov/` holds the
machine-readable sources those documents are generated from.

## Building

A recent stable Rust toolchain (edition 2024). The editor additionally needs Qt 6 and a
Vulkan-capable driver.

```sh
# the library crates — no system dependencies
cargo test --workspace --exclude strider-editor-qt

# the editor — Qt 6 required
cargo run -p strider-editor-qt
```

## License

Per-crate: the library crates are `Apache-2.0 OR MIT`, the application is
`AGPL-3.0-or-later`. Full texts live under [`LICENSES/`](LICENSES/);
[`LICENSE-APACHE`](LICENSE-APACHE) and [`LICENSE-MIT`](LICENSE-MIT) mirror the library
expression at the repository root. The tree is annotated per the
[REUSE](https://reuse.software) specification.
