use tracing_subscriber::prelude::*;
use tracing_unity::{Config, Runtime};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut runtime = Runtime::new(Config::default())?;
    let subscriber = tracing_subscriber::registry().with(runtime.layer());
    let dispatch = tracing::Dispatch::new(subscriber);
    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!(target: "example", items = 3, "ready");
    });
    // A host may instead expose the borrowed helpers in tracing_unity::abi.
    while let Some(record) = runtime
        .drain_record(2176)
        .map_err(|e| format!("poll failed: {e:?}"))?
    {
        println!("{:?} {} {}", record.level, record.target, record.message);
    }
    runtime.shutdown()?;
    Ok(())
}
