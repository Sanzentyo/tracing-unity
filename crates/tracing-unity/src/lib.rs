//! DLL-owned tracing dispatch and bounded sinks. Never installs a global subscriber.
//! Events retain level, target and event fields; span fields are not serialized.
//! Producers never wait for queue space or perform I/O. This is not a hard real-time API:
//! field formatting and tracing's own internals may execute arbitrary caller code.
//! Dispatch, spans, callsites and metadata must stay inside the same statically linked
//! image. Export only copied records across the DLL ABI, never tracing types.
//! Panic isolation requires unwinding; `panic=abort` terminates the process instead.
//! Never shut down or drop a runtime from DllMain, TLS destructors, or under a loader lock.
pub mod abi;

use std::{
    collections::VecDeque,
    fmt::{self, Write as _},
    fs::OpenOptions,
    io::{self, Write},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};
use tracing::{
    Dispatch, Event, Level, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

pub const MAX_TARGET_BYTES: usize = 128;
pub const MAX_MESSAGE_BYTES: usize = 2048;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RecordLevel {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}
impl From<&Level> for RecordLevel {
    fn from(v: &Level) -> Self {
        match *v {
            Level::ERROR => Self::Error,
            Level::WARN => Self::Warn,
            Level::INFO => Self::Info,
            Level::DEBUG => Self::Debug,
            _ => Self::Trace,
        }
    }
}
#[derive(Debug, Clone)]
pub struct Record {
    pub level: RecordLevel,
    pub target: String,
    pub message: String,
}
impl Record {
    /// Payload size: UTF-8 target bytes followed by UTF-8 message bytes, without NUL or header.
    pub fn payload_bytes(&self) -> usize {
        self.target.len() + self.message.len()
    }
}
#[derive(Debug)]
pub struct Config {
    pub file_path: Option<PathBuf>,
    /// Standalone hosts may retain stderr diagnostics, on the writer thread.
    pub stderr_when_no_file: bool,
    pub poll_enabled: bool,
    pub poll_capacity: usize,
    pub file_capacity: usize,
    pub max_level: Level,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            file_path: None,
            stderr_when_no_file: false,
            poll_enabled: true,
            poll_capacity: 256,
            file_capacity: 1024,
            max_level: Level::INFO,
        }
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    /// Records rejected by an enabled polling sink, including after closure.
    pub poll_dropped: u64,
    /// Records rejected by an enabled output sink, including after closure.
    pub file_dropped: u64,
    pub format_failed: u64,
    pub file_errors: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainError {
    BufferTooSmall { required: usize },
    Busy,
    Poisoned,
}
struct Queues {
    active: bool,
    poll: VecDeque<Record>,
    file: VecDeque<Record>,
}
struct Shared {
    queues: Mutex<Queues>,
    wake: Condvar,
    poll_capacity: usize,
    file_capacity: usize,
    file_enabled: bool,
    poll_enabled: bool,
    poll_dropped: AtomicU64,
    file_dropped: AtomicU64,
    format_failed: AtomicU64,
    file_errors: AtomicU64,
}
impl Shared {
    fn stats(&self) -> Stats {
        Stats {
            poll_dropped: self.poll_dropped.load(Ordering::Relaxed),
            file_dropped: self.file_dropped.load(Ordering::Relaxed),
            format_failed: self.format_failed.load(Ordering::Relaxed),
            file_errors: self.file_errors.load(Ordering::Relaxed),
        }
    }
    fn enqueue(&self, record: Record) {
        // Allocate outside the shared queue lock, keeping writer/consumer contention brief.
        let file_record = self.file_enabled.then(|| record.clone());
        let Ok(mut q) = self.queues.try_lock() else {
            if self.poll_enabled {
                self.poll_dropped.fetch_add(1, Ordering::Relaxed);
            }
            if self.file_enabled {
                self.file_dropped.fetch_add(1, Ordering::Relaxed);
            }
            return;
        };
        if !q.active {
            if self.poll_enabled {
                self.poll_dropped.fetch_add(1, Ordering::Relaxed);
            }
            if self.file_enabled {
                self.file_dropped.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        if let Some(file_record) = file_record {
            if q.file.len() < self.file_capacity {
                q.file.push_back(file_record);
                self.wake.notify_one();
            } else {
                self.file_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        if !self.poll_enabled {
            return;
        }
        if q.poll.len() < self.poll_capacity {
            q.poll.push_back(record);
        } else {
            self.poll_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
/// Composable event sink. Obtain one from [`Runtime::layer`].
/// Its level limit only affects this sink, never sibling layers.
pub struct EventLayer {
    shared: Arc<Shared>,
    max_level: Level,
}
impl<S: Subscriber> Layer<S> for EventLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() > self.max_level {
            return;
        }
        let formatted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut visitor = EventVisitor(BoundedText::new(MAX_MESSAGE_BYTES));
            event.record(&mut visitor);
            Record {
                level: event.metadata().level().into(),
                target: bounded(event.metadata().target(), MAX_TARGET_BYTES),
                message: visitor.0.text,
            }
        }));
        match formatted {
            Ok(record) => self.shared.enqueue(record),
            Err(_) => {
                self.shared.format_failed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
struct BoundedText {
    text: String,
    limit: usize,
    full: bool,
}
impl BoundedText {
    fn new(limit: usize) -> Self {
        Self {
            text: String::with_capacity(limit),
            limit,
            full: false,
        }
    }
}
impl fmt::Write for BoundedText {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.full {
            return Ok(());
        }
        let remaining = self.limit - self.text.len();
        let mut end = s.len().min(remaining);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&s[..end]);
        if end < s.len() {
            self.full = true;
        }
        Ok(())
    }
}
fn bounded(s: &str, limit: usize) -> String {
    let mut out = BoundedText::new(limit);
    let _ = out.write_str(s);
    out.text
}
struct EventVisitor(BoundedText);
impl Visit for EventVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if !self.0.text.is_empty() {
            let _ = self.0.write_char(' ');
        }
        if field.name() != "message" {
            let _ = write!(self.0, "{}=", field.name());
        }
        let _ = write!(self.0, "{value:?}");
    }
}

/// Owner must outlive every callback using its dispatch, and call shutdown before DLL unload.
/// Shutdown joins the writer even if dispatch clones survive. Callers must also retire their
/// own callback/worker threads before unloading any DLL code.
pub struct Runtime {
    shared: Arc<Shared>,
    dispatch: OnceLock<Dispatch>,
    writer: Option<JoinHandle<io::Result<()>>>,
    max_level: Level,
}
impl Runtime {
    /// Opens the optional sink once. Invalid path or thread spawn fails explicitly; callers
    /// may retry with file_path=None and report the error through their host's logger.
    pub fn new(config: Config) -> io::Result<Self> {
        if config.poll_capacity == 0 || config.file_capacity == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "logging queue capacities must be nonzero",
            ));
        }
        let sink: Option<Box<dyn Write + Send>> = match &config.file_path {
            Some(path) => Some(Box::new(
                OpenOptions::new().create(true).append(true).open(path)?,
            )),
            None if config.stderr_when_no_file => Some(Box::new(io::stderr())),
            None => None,
        };
        Self::with_writer(config, sink)
    }
    fn with_writer(config: Config, sink: Option<Box<dyn Write + Send>>) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            queues: Mutex::new(Queues {
                active: true,
                poll: VecDeque::with_capacity(config.poll_capacity),
                file: VecDeque::with_capacity(config.file_capacity),
            }),
            wake: Condvar::new(),
            poll_capacity: config.poll_capacity,
            file_capacity: config.file_capacity,
            file_enabled: sink.is_some(),
            poll_enabled: config.poll_enabled,
            poll_dropped: AtomicU64::new(0),
            file_dropped: AtomicU64::new(0),
            format_failed: AtomicU64::new(0),
            file_errors: AtomicU64::new(0),
        });
        let writer = if let Some(sink) = sink {
            let state = shared.clone();
            Some(
                thread::Builder::new()
                    .name("tracing-output-writer".into())
                    .spawn(move || writer_loop(state, sink))?,
            )
        } else {
            None
        };
        Ok(Self {
            shared,
            dispatch: OnceLock::new(),
            writer,
            max_level: config.max_level,
        })
    }
    /// Creates a layer for a host-owned subscriber. Keep this owner alive until
    /// producers stop, then call shutdown even if the host retains the layer.
    pub fn layer(&self) -> EventLayer {
        EventLayer {
            shared: self.shared.clone(),
            max_level: self.max_level,
        }
    }
    /// Convenience dispatch, never installed globally or on any thread by this crate.
    /// Lazily constructed: layer-only use does not register an extra dispatch.
    pub fn dispatch(&self) -> Dispatch {
        self.dispatch
            .get_or_init(|| {
                Dispatch::new(
                    tracing_subscriber::registry().with(self.layer().with_filter(
                        tracing_subscriber::filter::LevelFilter::from_level(self.max_level),
                    )),
                )
            })
            .clone()
    }
    pub fn stats(&self) -> Stats {
        self.shared.stats()
    }
    /// Pops only if capacity covers target+message UTF-8 bytes; no header/NUL is counted.
    /// Zero capacity is always a non-consuming query, even for an empty record.
    /// No record is removed on BufferTooSmall. Single-consumer use is recommended.
    pub fn drain_record(&self, capacity: usize) -> Result<Option<Record>, DrainError> {
        let mut q = match self.shared.queues.try_lock() {
            Ok(q) => q,
            Err(std::sync::TryLockError::WouldBlock) => return Err(DrainError::Busy),
            Err(_) => return Err(DrainError::Poisoned),
        };
        if let Some(record) = q.poll.front() {
            let required = record.payload_bytes();
            if capacity == 0 || capacity < required {
                return Err(DrainError::BufferTooSmall { required });
            }
        }
        Ok(q.poll.pop_front())
    }
    /// Closes admission atomically with enqueue, drains accepted file records and flushes.
    /// May block on filesystem I/O; use only at teardown, never on a render callback.
    /// Never call under a loader lock, from DllMain, or from a TLS destructor.
    pub fn shutdown(&mut self) -> io::Result<()> {
        {
            let mut q = self.shared.queues.lock().unwrap_or_else(|e| e.into_inner());
            q.active = false;
            self.shared.wake.notify_all();
        }
        if let Some(writer) = self.writer.take() {
            writer
                .join()
                .map_err(|_| io::Error::other("tracing output writer panicked"))?
        } else {
            Ok(())
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if self.shutdown().is_err() {
            self.shared.file_errors.fetch_add(1, Ordering::Relaxed);
        }
    }
}
fn writer_loop(shared: Arc<Shared>, mut sink: Box<dyn Write + Send>) -> io::Result<()> {
    let mut first_error = None;
    loop {
        let record = {
            let mut q = shared.queues.lock().unwrap_or_else(|e| e.into_inner());
            while q.file.is_empty() && q.active {
                q = shared.wake.wait(q).unwrap_or_else(|e| e.into_inner());
            }
            match q.file.pop_front() {
                Some(r) => r,
                None => break,
            }
        };
        // Record fields are already bounded. A single write_all avoids the
        // per-format-fragment writes of writeln! directly against a File.
        // Dedicated paths are still required for cross-process ordering.
        let line = format!("{:?} {} {}\n", record.level, record.target, record.message);
        if let Err(e) = sink.write_all(line.as_bytes()) {
            shared.file_errors.fetch_add(1, Ordering::Relaxed);
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }
    if let Err(e) = sink.flush() {
        shared.file_errors.fetch_add(1, Ordering::Relaxed);
        if first_error.is_none() {
            first_error = Some(e);
        }
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn zero_capacity_query_preserves_even_empty_record() {
        let logger = runtime(2);
        logger.shared.enqueue(Record {
            level: RecordLevel::Info,
            target: String::new(),
            message: String::new(),
        });
        assert!(matches!(
            logger.drain_record(0),
            Err(DrainError::BufferTooSmall { required: 0 })
        ));
        assert!(logger.drain_record(1).expect("drain").is_some());
        assert!(logger.drain_record(1).expect("drain").is_none());
    }

    #[test]
    fn standalone_disables_unused_poll_queue() {
        let logger = Runtime::new(Config {
            poll_enabled: false,
            ..Config::default()
        })
        .expect("init");
        for _ in 0..300 {
            emit(&logger.dispatch());
        }
        assert!(logger.drain_record(2176).expect("drain").is_none());
        assert_eq!(logger.stats().poll_dropped, 0);
    }

    fn runtime(capacity: usize) -> Runtime {
        Runtime::new(Config {
            poll_capacity: capacity,
            ..Config::default()
        })
        .expect("logging init")
    }
    fn emit(dispatch: &Dispatch) {
        tracing::dispatcher::with_default(
            dispatch,
            || tracing::info!(target:"test",seq=17,"hello"),
        );
    }
    fn pop(runtime: &Runtime) -> Record {
        runtime
            .drain_record(MAX_TARGET_BYTES + MAX_MESSAGE_BYTES)
            .expect("drain")
            .expect("record")
    }
    #[test]
    fn structured_event_level_target_fields_and_short_buffer_retention() {
        let runtime = runtime(4);
        emit(&runtime.dispatch());
        let required = match runtime.drain_record(0) {
            Err(DrainError::BufferTooSmall { required }) => required,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(
            runtime.drain_record(required - 1).unwrap_err(),
            DrainError::BufferTooSmall { required }
        );
        let record = runtime.drain_record(required).unwrap().unwrap();
        assert_eq!(record.level, RecordLevel::Info);
        assert_eq!(record.target, "test");
        assert!(record.message.contains("seq=17"));
        assert!(record.message.contains("hello"));
        assert!(runtime.drain_record(0).unwrap().is_none());
    }
    #[test]
    fn utf8_formatting_is_bounded_without_splitting_characters() {
        let runtime = runtime(4);
        let message = "あ".repeat(4096);
        tracing::dispatcher::with_default(
            &runtime.dispatch(),
            || tracing::warn!(target:"utf8",payload=%message,"start"),
        );
        let record = pop(&runtime);
        assert!(record.message.len() <= MAX_MESSAGE_BYTES);
        assert!(record.message.is_char_boundary(record.message.len()));
        assert!(record.message.contains("payload="));
        assert_eq!(record.level, RecordLevel::Warn);
        assert_eq!(bounded("あいう", 5), "あ");
    }
    #[test]
    fn queue_overflow_preserves_oldest_and_counts_loss() {
        let runtime = runtime(1);
        let dispatch = runtime.dispatch();
        emit(&dispatch);
        emit(&dispatch);
        assert_eq!(runtime.stats().poll_dropped, 1);
        assert_eq!(runtime.stats().file_dropped, 0);
        let _ = pop(&runtime);
        assert!(runtime.drain_record(0).unwrap().is_none());
    }
    #[test]
    fn contention_drops_instead_of_waiting() {
        let runtime = runtime(4);
        let _guard = runtime.shared.queues.lock().unwrap();
        emit(&runtime.dispatch());
        assert_eq!(runtime.stats().poll_dropped, 1);
        assert_eq!(runtime.drain_record(4096).unwrap_err(), DrainError::Busy);
    }
    #[test]
    fn no_global_dispatch_and_explicit_worker_dispatch() {
        let runtime = runtime(4);
        let dispatch = runtime.dispatch();
        thread::spawn(|| tracing::info!("not captured"))
            .join()
            .unwrap();
        thread::spawn(move || emit(&dispatch)).join().unwrap();
        assert_eq!(pop(&runtime).target, "test");
        assert!(runtime.drain_record(0).unwrap().is_none());
    }
    #[test]
    fn closed_owner_counts_surviving_dispatch_and_new_owner_is_independent() {
        let mut first = runtime(4);
        let old = first.dispatch();
        emit(&old);
        first.shutdown().unwrap();
        emit(&old);
        assert_eq!(pop(&first).target, "test");
        assert_eq!(first.stats().poll_dropped, 1);
        assert_eq!(first.stats().file_dropped, 0);
        assert!(first.drain_record(4096).unwrap().is_none());
        first.shutdown().unwrap();
        let second = runtime(4);
        emit(&second.dispatch());
        assert_eq!(pop(&second).target, "test");
    }
    #[test]
    fn layer_only_use_leaves_convenience_dispatch_uninitialized() {
        let runtime = runtime(4);
        assert!(runtime.dispatch.get().is_none());
        let host = Dispatch::new(tracing_subscriber::registry().with(runtime.layer()));
        emit(&host);
        assert_eq!(pop(&runtime).target, "test");
        assert!(runtime.dispatch.get().is_none());
        drop(host);
        let convenience = runtime.dispatch();
        assert!(runtime.dispatch.get().is_some());
        tracing::dispatcher::with_default(&convenience, || {
            assert!(!tracing::enabled!(Level::DEBUG));
            assert!(tracing::enabled!(Level::INFO));
        });
    }
    #[test]
    fn closed_queue_counts_only_enabled_sinks() {
        for poll_enabled in [false, true] {
            for file_enabled in [false, true] {
                let config = Config {
                    poll_enabled,
                    ..Config::default()
                };
                let sink = file_enabled.then(|| Box::new(io::sink()) as Box<dyn Write + Send>);
                let mut runtime = Runtime::with_writer(config, sink).unwrap();
                let dispatch = runtime.dispatch();
                runtime.shutdown().unwrap();
                emit(&dispatch);
                assert_eq!(runtime.stats().poll_dropped, u64::from(poll_enabled));
                assert_eq!(runtime.stats().file_dropped, u64::from(file_enabled));
                assert!(runtime.drain_record(4096).unwrap().is_none());
            }
        }
    }
    #[test]
    fn level_filter_rejects_verbose_events() {
        let runtime = runtime(4);
        tracing::dispatcher::with_default(&runtime.dispatch(), || {
            tracing::debug!("hidden");
            tracing::error!("visible");
        });
        assert_eq!(pop(&runtime).level, RecordLevel::Error);
        assert!(runtime.drain_record(0).unwrap().is_none());
    }
    struct PanicDebug;
    impl fmt::Debug for PanicDebug {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            panic!("bad caller Debug")
        }
    }
    #[test]
    fn faulty_caller_formatter_is_counted_without_unwinding() {
        let runtime = runtime(4);
        tracing::dispatcher::with_default(&runtime.dispatch(), || tracing::info!(bad=?PanicDebug));
        assert_eq!(runtime.stats().format_failed, 1);
        assert!(runtime.drain_record(0).unwrap().is_none());
    }
    struct Sink {
        output: Arc<Mutex<Vec<u8>>>,
        flushed: Arc<AtomicU64>,
    }
    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    #[test]
    fn shutdown_drains_file_and_flushes_with_dispatch_still_alive() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let flushed = Arc::new(AtomicU64::new(0));
        let mut runtime = Runtime::with_writer(
            Config::default(),
            Some(Box::new(Sink {
                output: output.clone(),
                flushed: flushed.clone(),
            })),
        )
        .unwrap();
        let dispatch = runtime.dispatch();
        // Seed an accepted record atomically, independent of writer scheduling.
        {
            let mut q = runtime.shared.queues.lock().unwrap();
            q.file.push_back(Record {
                level: RecordLevel::Info,
                target: "test".into(),
                message: "hello".into(),
            });
            runtime.shared.wake.notify_one();
        }
        runtime.shutdown().unwrap();
        let bytes = output.lock().unwrap().clone();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text, "Info test hello\n");
        assert_eq!(flushed.load(Ordering::Relaxed), 1);
        let before = output.lock().unwrap().len();
        emit(&dispatch);
        assert_eq!(output.lock().unwrap().len(), before);
    }
    struct FailingSink;
    impl Write for FailingSink {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("write failed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }
    #[test]
    fn io_error_is_reported_at_shutdown_and_in_stats() {
        let mut runtime =
            Runtime::with_writer(Config::default(), Some(Box::new(FailingSink))).unwrap();
        emit(&runtime.dispatch());
        assert!(runtime.shutdown().is_err());
        assert!(runtime.stats().file_errors >= 1);
    }
    struct GatedSink {
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        first: bool,
    }
    impl Write for GatedSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.first {
                self.first = false;
                self.entered.send(()).unwrap();
                self.release.recv().unwrap();
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn blocked_file_does_not_block_polling_and_file_queue_is_bounded() {
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let mut runtime = Runtime::with_writer(
            Config {
                file_capacity: 1,
                ..Config::default()
            },
            Some(Box::new(GatedSink {
                entered: entered_tx,
                release: release_rx,
                first: true,
            })),
        )
        .unwrap();
        // Enqueue before writer startup can acquire its lock, using the same admission lock.
        {
            let mut q = runtime.shared.queues.lock().unwrap();
            q.file.push_back(Record {
                level: RecordLevel::Info,
                target: "test".into(),
                message: "first".into(),
            });
            runtime.shared.wake.notify_one();
        }
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        emit(&runtime.dispatch());
        emit(&runtime.dispatch());
        assert_eq!(runtime.stats().file_dropped, 1);
        assert_eq!(pop(&runtime).target, "test");
        release_tx.send(()).unwrap();
        runtime.shutdown().unwrap();
    }
    #[test]
    fn bad_path_and_zero_capacity_return_errors() {
        assert!(
            Runtime::new(Config {
                poll_capacity: 0,
                ..Config::default()
            })
            .is_err()
        );
        let path = std::env::temp_dir()
            .join(format!("nonexistent-log-parent-{}", std::process::id()))
            .join("log.txt");
        assert!(
            Runtime::new(Config {
                file_path: Some(path),
                ..Config::default()
            })
            .is_err()
        );
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use std::sync::mpsc;
    struct GatedRecordingSink {
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        first: bool,
        bytes: Arc<Mutex<Vec<u8>>>,
        flushed: Arc<AtomicU64>,
        dropped: Arc<AtomicU64>,
    }
    impl Write for GatedRecordingSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.first {
                self.first = false;
                self.entered.send(()).unwrap();
                self.release.recv().unwrap();
            }
            self.bytes.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    impl Drop for GatedRecordingSink {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    #[test]
    fn shutdown_waits_for_writer_drains_in_order_and_retires_sink() {
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let flushed = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let mut runtime = Runtime::with_writer(
            Config::default(),
            Some(Box::new(GatedRecordingSink {
                entered: entered_tx,
                release: release_rx,
                first: true,
                bytes: bytes.clone(),
                flushed: flushed.clone(),
                dropped: dropped.clone(),
            })),
        )
        .unwrap();
        {
            let mut q = runtime.shared.queues.lock().unwrap();
            q.file.push_back(Record {
                level: RecordLevel::Info,
                target: "test".into(),
                message: "first".into(),
            });
            runtime.shared.wake.notify_one();
        }
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let dispatch = runtime.dispatch();
        tracing::dispatcher::with_default(&dispatch, || {
            tracing::info!(target:"test","second");
            tracing::info!(target:"test","third");
        });
        assert_eq!(runtime.stats().file_dropped, 0);
        let shared = runtime.shared.clone();
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let closer = thread::spawn(move || {
            let result = runtime.shutdown();
            done_tx.send(result).unwrap();
        });
        // Observe the actual close linearization; the writer remains held at the test barrier.
        loop {
            if !shared.queues.lock().unwrap().active {
                break;
            }
            thread::yield_now();
        }
        assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        tracing::dispatcher::with_default(&dispatch, || tracing::info!(target:"test","too late"));
        release_tx.send(()).unwrap();
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        closer.join().unwrap();
        assert_eq!(
            String::from_utf8(bytes.lock().unwrap().clone()).unwrap(),
            "Info test first\nInfo test second\nInfo test third\n"
        );
        assert_eq!(flushed.load(Ordering::Relaxed), 1);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn empty_writer_shutdown_flushes_once() {
        let mut runtime =
            Runtime::with_writer(Config::default(), Some(Box::new(io::sink()))).unwrap();
        runtime.shutdown().unwrap();
        runtime.shutdown().unwrap();
        assert_eq!(runtime.stats().file_errors, 0);
    }
}
