use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tracing::{Dispatch, Event, Subscriber};
use tracing_subscriber::{Layer, layer::Context, prelude::*};
use tracing_unity::{Config, Runtime};

struct Count(Arc<AtomicU64>);
impl<S: Subscriber> Layer<S> for Count {
    fn on_event(&self, _: &Event<'_>, _: Context<'_, S>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
fn emit_pair() {
    tracing::debug!(target:"example","verbose");
    tracing::info!(target:"example","visible");
}

#[test]
fn sink_level_filter_does_not_suppress_sibling_layers() {
    for reverse in [false, true] {
        let mut runtime = Runtime::new(Config::default()).unwrap();
        let count = Arc::new(AtomicU64::new(0));
        let dispatch = if reverse {
            Dispatch::new(
                tracing_subscriber::registry()
                    .with(Count(count.clone()))
                    .with(runtime.layer()),
            )
        } else {
            Dispatch::new(
                tracing_subscriber::registry()
                    .with(runtime.layer())
                    .with(Count(count.clone())),
            )
        };
        tracing::dispatcher::with_default(&dispatch, emit_pair);
        assert_eq!(count.load(Ordering::Relaxed), 2);
        assert!(
            runtime
                .drain_record(2176)
                .unwrap()
                .unwrap()
                .message
                .contains("visible")
        );
        assert!(runtime.drain_record(2176).unwrap().is_none());
        runtime.shutdown().unwrap();
        tracing::dispatcher::with_default(&dispatch, emit_pair);
        assert_eq!(count.load(Ordering::Relaxed), 4);
        assert!(runtime.drain_record(2176).unwrap().is_none());
    }
}

#[test]
fn creating_runtime_preserves_host_scoped_dispatch() {
    let count = Arc::new(AtomicU64::new(0));
    let host = Dispatch::new(tracing_subscriber::registry().with(Count(count.clone())));
    tracing::dispatcher::with_default(&host, || {
        let mut runtime = Runtime::new(Config::default()).unwrap();
        emit_pair();
        assert_eq!(count.load(Ordering::Relaxed), 2);
        assert!(runtime.drain_record(2176).unwrap().is_none());
        runtime.shutdown().unwrap();
        emit_pair();
        assert_eq!(count.load(Ordering::Relaxed), 4);
    });
}
