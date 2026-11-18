use tracing::Level;
use tracing_unity::{Config, RecordLevel, Runtime, SetLevelError, abi};

fn emit_all() {
    tracing::error!("error");
    tracing::warn!("warn");
    tracing::info!("info");
    tracing::debug!("debug");
    tracing::trace!("trace");
}
fn levels(runtime: &Runtime) -> Vec<u32> {
    let mut result = Vec::new();
    loop {
        match runtime.drain_record(2176) {
            Ok(Some(record)) => result.push(record.level as u32),
            Ok(None) => return result,
            Err(tracing_unity::DrainError::Busy) => std::thread::yield_now(),
            Err(error) => panic!("unexpected drain error: {error:?}"),
        }
    }
}
#[test]
fn all_levels_and_same_callsites_change_without_cache_loss() {
    let mut runtime = Runtime::new(Config::default()).unwrap();
    let dispatch = runtime.dispatch();
    for (level, count) in [
        (Level::INFO, 3),
        (Level::DEBUG, 4),
        (Level::INFO, 3),
        (Level::TRACE, 5),
        (Level::ERROR, 1),
        (Level::WARN, 2),
    ] {
        runtime.set_poll_max_level(level).unwrap();
        assert_eq!(runtime.poll_max_level(), level);
        tracing::dispatcher::with_default(&dispatch, emit_all);
        assert_eq!(levels(&runtime), (1..=count).collect::<Vec<_>>());
    }
    runtime.shutdown().unwrap();
    assert_eq!(
        runtime.set_poll_max_level(Level::TRACE),
        Err(SetLevelError::Closed)
    );
    assert_eq!(runtime.poll_max_level(), Level::WARN);
}
#[test]
fn numeric_abi_validates_without_changing_on_error() {
    let mut runtime = Runtime::new(Config::default()).unwrap();
    for level in 1..=5 {
        assert_eq!(abi::set_poll_max_level(&runtime, level), 0);
        assert_eq!(abi::poll_max_level(&runtime), level);
    }
    for invalid in [0, 6, u32::MAX] {
        assert_eq!(abi::set_poll_max_level(&runtime, invalid), -1);
        assert_eq!(abi::poll_max_level(&runtime), 5);
    }
    runtime.shutdown().unwrap();
    assert_eq!(abi::set_poll_max_level(&runtime, 3), -4);
    assert_eq!(abi::poll_max_level(&runtime), 5);
}
#[test]
fn existing_queue_is_not_purged_and_initial_override_is_independent() {
    let runtime = Runtime::new(Config {
        max_level: Level::ERROR,
        poll_max_level: Some(Level::TRACE),
        ..Config::default()
    })
    .unwrap();
    tracing::dispatcher::with_default(&runtime.dispatch(), emit_all);
    runtime.set_poll_max_level(Level::ERROR).unwrap();
    assert_eq!(levels(&runtime), vec![1, 2, 3, 4, 5]);
    let legacy = Runtime::new(Config {
        max_level: Level::WARN,
        ..Config::default()
    })
    .unwrap();
    assert_eq!(legacy.poll_max_level(), Level::WARN);
    assert_eq!(RecordLevel::from(&legacy.poll_max_level()) as u32, 2);
}

#[test]
fn convenience_rejects_spans_and_skips_filtered_field_expressions() {
    let runtime = Runtime::new(Config::default()).unwrap();
    let dispatch = runtime.dispatch();
    let evaluated = std::cell::Cell::new(0);
    tracing::dispatcher::with_default(&dispatch, || {
        assert!(
            tracing::trace_span!(
                "unused",
                value = {
                    evaluated.set(evaluated.get() + 1);
                    1
                }
            )
            .is_disabled()
        );
        assert!(tracing::info_span!("also_unused").is_disabled());
        for (level, expected) in [(Level::INFO, 0), (Level::TRACE, 1), (Level::INFO, 1)] {
            runtime.set_poll_max_level(level).unwrap();
            tracing::trace!(
                value = {
                    evaluated.set(evaluated.get() + 1);
                    1
                },
                "expensive"
            );
            assert_eq!(evaluated.get(), expected);
        }
    });
    assert_eq!(levels(&runtime), vec![5]);
}
