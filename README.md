# tracing-unity

Bounded delivery of Rust `tracing` events to a Unity main-thread log pump, with an optional asynchronous file or stderr sink.

## Components

- `crates/tracing-unity`: a Rust runtime, composable `EventLayer`, and copied-record C ABI helpers. Statically link this crate into each native plugin.
- `Packages/com.tracing-unity.logging`: a Unity package exposing `ILogSource` and the instance-owned `LogPump` component. The host supplies its own P/Invoke adapter and native lifecycle.

The library never installs a process-global subscriber. A host that installs a layer globally must keep that Runtime for the whole process, or supply its own safe generation-routing layer. A closed Runtime does not rebind to a new Play session; retained global/thread-local Dispatch clones continue to target the old closed queue. Recreate scoped dispatches and retire old guards explicitly. Attach its layer to a host-owned subscriber, or use the optional local `Dispatch` convenience API. Its level filter applies only to this layer and does not suppress sibling layers.

```rust
use tracing_unity::{Config, Runtime};
use tracing_subscriber::prelude::*;

let mut runtime = Runtime::new(Config::default())?;
let subscriber = tracing_subscriber::registry().with(runtime.layer());
let dispatch = tracing::Dispatch::new(subscriber);
tracing::dispatcher::with_default(&dispatch, || tracing::info!(answer = 42, "ready"));
// Copy records through runtime.drain_record(...) or the ABI helpers.
runtime.shutdown()?;
# Ok::<(), std::io::Error>(())
```

The configuration accepts an explicit file path; it reads no application-specific environment variables. File open errors are returned, so the host can choose a poll-only fallback. Writer errors are observable in counters and at shutdown.

## Unity installation and use

Install the package from this repository's `Packages/com.tracing-unity.logging` path using Unity Package Manager. For a Git dependency, pin a full reviewed commit and use `?path=/Packages/com.tracing-unity.logging#<commit>`.

Implement `ILogSource.Poll` and `GetStats` as thin bindings to exports in your own plugin. On Unity's main thread, add a `LogPump`, then call `Attach(source)`. `LateUpdate` drains at most 32 records per invocation. Error and warning records retain their severity. Manual `Drain()` also has this limit and rejects reentrant calls; do not call it repeatedly to bypass the frame budget.

All pump APIs require Unity main-thread calls; Attach also requires a live active component. Destroyed or inactive pumps do not poll. The pump has no static registry and subscribes to no global Unity events. `OnDisable` detaches without touching native resources. Re-enabling or entering a new Play session requires an explicit fresh `Attach` by the owner. Do not reuse a stale source across native generations. The pump does not automatically discover or initialize plugins.

## Lifetime and safety

1. Keep the runtime, tracing callsites, registry and all Dispatch clones inside the same statically linked native image. Never pass Rust tracing objects across a DLL ABI.
2. Stop and join your native producers and quiesce callbacks before unmapping code. Also stop all polling and detach before freeing the Runtime handle; mutable shutdown must never alias an outstanding shared poll borrow. Closing the logging queue alone cannot make an outstanding callback safe.
3. Explicitly call `Runtime::shutdown` while the image remains mapped. It closes admission, drains accepted file records, flushes and joins the writer. It can block on filesystem I/O. It is not a real-time or bounded-duration operation. Never run shutdown or drop the owner under the OS loader lock (for example DllMain or a loader-managed TLS destructor).
4. Drain remaining managed-visible records as appropriate, detach the pump, and only then release the native image. Unity's last Update or process exit is not a guarantee of final native flushing. The source owner must arrange its own reliable shutdown path.

Surviving layers/dispatches cannot enqueue after shutdown. Rust `Drop` also closes the runtime, but static storage does not automatically run destructors at process exit. No forced-unload, crash preservation, arbitrary hot reload, or native Editor reinitialization guarantee is provided.

Queues and strings are bounded. Producers use try-lock and drop new records under contention or saturation, with counters. Panic isolation requires an unwinding build; panic=abort terminates the process. Formatting and tracing internals may execute caller code: this is not a lock-free or hard real-time API. Event fields are flattened into text; span fields are not serialized. File size is not rotated or bounded. Use separate file paths per process/session and implement retention outside the library.

## ABI

See [ABI.md](ABI.md) for byte layout, ownership and status codes. The library exports no global C symbols: your plugin supplies wrappers around borrowed-runtime helpers. The managed package deliberately contains no fixed native DLL name.

## Development

Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo nextest run --workspace`. Run `cargo run --example composed` to exercise the Layer composition example; `cargo test --doc` checks any Rust doctests (currently none). The Unity package includes EditMode tests; add its name to a test project's `testables` list and install Unity Test Framework. Native plugin lifetime tests remain the integrating application's responsibility.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE), and [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
