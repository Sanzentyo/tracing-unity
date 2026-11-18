# Copied-record ABI

`LogHeader` is four sequential `uint32_t` values: `bytes` (16), `level`, `target_bytes`, `message_bytes`. Levels: 1 error, 2 warning, 3 info, 4 debug, 5 trace.

The payload is UTF-8 target bytes followed by UTF-8 message bytes, without a terminator. Embedded NUL is allowed. Target capacity is 128 bytes and message capacity is 2048 bytes, truncated at code-point boundaries. A 2176-byte buffer holds any record.

Poll status codes:

- 1: one record copied and consumed.
- 0: queue empty.
- -1: invalid arguments or poisoned state.
- -2: insufficient capacity; record retained. Header target length is zero and message length contains the required combined capacity.
- -3: temporary contention; retry on a later frame.

Zero capacity is always a non-consuming query, including an empty payload. Raw helpers accept null buffer only with zero capacity. Headers and stats must be writable and properly aligned; output regions must not overlap. The host must provide valid lifetime/exclusive access for all pointers and runtime references. No pointer validation can prove an arbitrary foreign pointer is valid.

`LogStats` is four sequential `uint64_t` values, 32 bytes total: poll drops, file drops, formatting failures, file errors. Stats are cumulative. The host should use Cdecl for exports bound by managed code and catch any additional host-specific panic at its export boundary.

The native runtime borrows no managed buffer after a call returns. No function pointer, Rust string, trait object or tracing metadata crosses this ABI. Native owner locking, initialization and teardown are host responsibilities, not hidden global behavior in the helper.

The Unity pump displays embedded NUL as the visible sequence \\0 because Unity native log strings otherwise stop at NUL.

## Runtime polling threshold

`Runtime::set_poll_max_level(Level)` changes polling admission only. `poll_max_level()` reads the current typed threshold. `Config::max_level` retains its file/stderr threshold and initial polling meaning; `Config::poll_max_level: Option<Level>` optionally overrides the initial polling threshold (default `None`, so the default threshold remains INFO). Existing explicit all-field `Config` literals must add the new field; literals using `..Config::default()` need no change.

The borrowed helper `abi::set_poll_max_level(&Runtime, u32)` accepts 1..5 using the levels above, returning 0 on success, -1 for invalid input or poisoned state, and -4 for a closed runtime. Invalid or rejected changes leave the threshold unchanged. `abi::poll_max_level(&Runtime)` returns 1..5, also after closure. No new exported C symbol or runtime ownership is supplied by the crate.

Changes are serialized with closure and queue admission. Already queued records remain available. Concurrent events may be filtered before an increase or at admission after a decrease; no replay is promised. Level filtering does not increment drop counters. Setter calls can wait for the queue mutex and belong on the control path, not inside an event formatter. The convenience dispatch keeps TRACE callsites eligible for later increases. A host-composed subscriber must likewise avoid stricter upstream/static filters if it expects dynamic increases; this library cannot override host or compile-time tracing filters. File/stderr and sibling layer thresholds are unchanged.
