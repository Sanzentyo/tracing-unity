// This binary has exactly one test because a global subscriber cannot be reset.
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tracing::{Event, Subscriber};
use tracing_subscriber::{Layer, layer::Context, prelude::*};
use tracing_unity::{Config, Runtime};
struct Count(Arc<AtomicU64>);
impl<S: Subscriber> Layer<S> for Count {
    fn on_event(&self, _: &Event<'_>, _: Context<'_, S>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
#[test]
fn runtime_never_replaces_host_global_subscriber() {
    let count = Arc::new(AtomicU64::new(0));
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(Count(count.clone())),
    )
    .unwrap();
    let mut runtime = Runtime::new(Config::default()).unwrap();
    tracing::info!("host-owned event");
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert!(runtime.drain_record(2176).unwrap().is_none());
    tracing::dispatcher::with_default(&runtime.dispatch(), || {
        tracing::info!("explicit local event")
    });
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert!(runtime.drain_record(2176).unwrap().is_some());
    runtime.shutdown().unwrap();
    tracing::info!("still host-owned");
    assert_eq!(count.load(Ordering::Relaxed), 2);
}
