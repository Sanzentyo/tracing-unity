# Third-party notices

This project depends on the following Rust packages through Cargo. Their code is not vendored. Preserve applicable license notices when redistributing dependency code or binaries.

| Package | License |
| --- | --- |
| tracing, tracing-core, tracing-subscriber | MIT |
| sharded-slab | MIT |
| cfg-if, lazy_static, once_cell, thread_local | MIT OR Apache-2.0 |
| pin-project-lite | Apache-2.0 OR MIT |

The tracing family is copyright Tokio Contributors. Upstream license files are distributed with the resolved Cargo packages. `Cargo.lock` records the exact versions used for development; downstream dependency resolution may differ.

Unity and Unity Test Framework are separately provided tools/packages, not vendored here. This repository includes no Unity binaries, native engine headers, or third-party source implementation.
