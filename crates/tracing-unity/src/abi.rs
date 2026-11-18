//! Version-1 C-compatible records and borrowed polling helpers.
//!
//! The host owns the runtime, exported function names, initialization and shutdown.
//! These helpers do not export symbols, store global state or transfer tracing types.
use crate::{DrainError, RecordLevel, Runtime, SetLevelError};

pub const POLL_RECORD: i32 = 1;
pub const POLL_EMPTY: i32 = 0;
pub const POLL_INVALID: i32 = -1;
pub const POLL_SHORT_BUFFER: i32 = -2;
pub const POLL_BUSY: i32 = -3;
pub const LEVEL_CLOSED: i32 = -4;

/// Changes only polling admission: 1=error, 2=warn, 3=info, 4=debug, 5=trace.
/// Returns 0 on success, -1 for invalid input/poisoned state, -4 after closure.
/// Invalid input never changes the threshold. No foreign pointers are retained.
pub fn set_poll_max_level(runtime: &Runtime, level: u32) -> i32 {
    let level = match level {
        1 => tracing::Level::ERROR,
        2 => tracing::Level::WARN,
        3 => tracing::Level::INFO,
        4 => tracing::Level::DEBUG,
        5 => tracing::Level::TRACE,
        _ => return POLL_INVALID,
    };
    match runtime.set_poll_max_level(level) {
        Ok(()) => 0,
        Err(SetLevelError::Closed) => LEVEL_CLOSED,
        Err(SetLevelError::Poisoned) => POLL_INVALID,
    }
}

/// Returns the current polling threshold as 1..=5, including after shutdown.
pub fn poll_max_level(runtime: &Runtime) -> u32 {
    RecordLevel::from(&runtime.poll_max_level()) as u32
}

/// Header size is 16 bytes. Payload is target UTF-8 followed by message UTF-8,
/// without NUL terminators. `level` is 1=error, 2=warn, 3=info, 4=debug, 5=trace.
/// On a short-buffer result, `target_bytes=0`, `message_bytes=required capacity`.
/// An empty record reports zero payload bytes; pass a nonempty buffer to consume
/// it, since zero capacity is reserved for a non-consuming query.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogHeader {
    pub bytes: u32,
    pub level: u32,
    pub target_bytes: u32,
    pub message_bytes: u32,
}

/// Cumulative counters. Layout is four consecutive 64-bit unsigned integers.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogStats {
    pub poll_dropped: u64,
    pub file_dropped: u64,
    pub format_failed: u64,
    pub file_errors: u64,
}

/// Polls at most one record. An empty buffer queries the required size without
/// consuming a record, including a record whose payload is empty.
/// Busy and short-buffer results never consume a record.
pub fn poll(runtime: &Runtime, buffer: &mut [u8], header: &mut LogHeader) -> i32 {
    *header = LogHeader {
        bytes: size_of::<LogHeader>() as u32,
        ..LogHeader::default()
    };
    match runtime.drain_record(buffer.len()) {
        Ok(Some(record)) => {
            let target = record.target.len();
            let message = record.message.len();
            buffer[..target].copy_from_slice(record.target.as_bytes());
            buffer[target..target + message].copy_from_slice(record.message.as_bytes());
            *header = LogHeader {
                bytes: size_of::<LogHeader>() as u32,
                level: record.level as u32,
                target_bytes: target as u32,
                message_bytes: message as u32,
            };
            POLL_RECORD
        }
        Ok(None) => POLL_EMPTY,
        Err(DrainError::BufferTooSmall { required }) => {
            header.message_bytes = required as u32;
            POLL_SHORT_BUFFER
        }
        Err(DrainError::Busy) => POLL_BUSY,
        Err(DrainError::Poisoned) => POLL_INVALID,
    }
}

/// Copies the current counters without exposing runtime ownership.
pub fn stats(runtime: &Runtime) -> LogStats {
    let value = runtime.stats();
    LogStats {
        poll_dropped: value.poll_dropped,
        file_dropped: value.file_dropped,
        format_failed: value.format_failed,
        file_errors: value.file_errors,
    }
}

/// Raw-pointer adapter for a host's C export. Null buffer with zero capacity is
/// allowed for a non-consuming size query. Rust panics are translated to -1.
///
/// # Safety
/// `header` must be valid, aligned and exclusively writable for one LogHeader.
/// For nonzero capacity, `buffer` must be valid and exclusively writable for
/// that many bytes. The output regions must not overlap each other or runtime
/// storage. Their borrows end when this call returns. The runtime must remain
/// live throughout the call. Null and misaligned headers are rejected.
pub unsafe fn poll_raw(
    runtime: &Runtime,
    buffer: *mut u8,
    capacity: u32,
    header: *mut LogHeader,
) -> i32 {
    if header.is_null()
        || !header.is_aligned()
        || (buffer.is_null() && capacity != 0)
        || u64::from(capacity) > isize::MAX as u64
    {
        return POLL_INVALID;
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: caller supplies disjoint, valid, exclusively borrowed outputs.
        // A null zero-length buffer is handled without constructing a null slice.
        let output = if capacity == 0 {
            &mut []
        } else {
            unsafe { std::slice::from_raw_parts_mut(buffer, capacity as usize) }
        };
        unsafe { poll(runtime, output, &mut *header) }
    }))
    .unwrap_or(POLL_INVALID)
}

/// Raw-pointer adapter for a host's C export. Returns 0, or -1 for invalid output.
///
/// # Safety
/// `out` must be valid, aligned and exclusively writable for one LogStats and
/// must not alias runtime storage. Null and misaligned pointers are rejected.
pub unsafe fn stats_raw(runtime: &Runtime, out: *mut LogStats) -> i32 {
    if out.is_null() || !out.is_aligned() {
        return POLL_INVALID;
    }
    // SAFETY: the caller grants exclusive valid output storage.
    unsafe {
        out.write(stats(runtime));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    #[test]
    fn layouts_and_invalid_outputs() {
        assert_eq!(size_of::<LogHeader>(), 16);
        assert_eq!(std::mem::offset_of!(LogHeader, level), 4);
        assert_eq!(std::mem::offset_of!(LogHeader, target_bytes), 8);
        assert_eq!(std::mem::offset_of!(LogHeader, message_bytes), 12);
        assert_eq!(size_of::<LogStats>(), 32);
        assert_eq!(std::mem::offset_of!(LogStats, file_errors), 24);
        let runtime = Runtime::new(Config::default()).unwrap();
        let mut bytes = [0u8; 64];
        let mut header = LogHeader::default();
        // SAFETY: invalid arguments are explicitly rejected without dereference.
        unsafe {
            assert_eq!(
                poll_raw(&runtime, std::ptr::null_mut(), 1, &mut header),
                POLL_INVALID
            );
            assert_eq!(
                poll_raw(&runtime, bytes.as_mut_ptr(), 64, std::ptr::null_mut()),
                POLL_INVALID
            );
            let address = bytes.as_mut_ptr() as usize;
            let offset = (align_of::<LogHeader>() - address % align_of::<LogHeader>())
                % align_of::<LogHeader>()
                + 1;
            assert_eq!(
                poll_raw(
                    &runtime,
                    std::ptr::null_mut(),
                    0,
                    bytes.as_mut_ptr().add(offset).cast()
                ),
                POLL_INVALID
            );
            assert_eq!(stats_raw(&runtime, std::ptr::null_mut()), POLL_INVALID);
            let offset = (align_of::<LogStats>() - address % align_of::<LogStats>())
                % align_of::<LogStats>()
                + 1;
            assert_eq!(
                stats_raw(&runtime, bytes.as_mut_ptr().add(offset).cast()),
                POLL_INVALID
            );
        }
    }
    #[test]
    fn utf8_short_query_preserves_record_and_copies_only_payload() {
        let mut runtime = Runtime::new(Config::default()).unwrap();
        tracing::dispatcher::with_default(
            &runtime.dispatch(),
            || tracing::warn!(target:"example", "日本語🙂\0end"),
        );
        let mut header = LogHeader::default();
        let mut buffer = [0xAAu8; 2176];
        // SAFETY: valid disjoint outputs; a null zero-capacity query is supported.
        unsafe {
            assert_eq!(
                poll_raw(&runtime, std::ptr::null_mut(), 0, &mut header),
                POLL_SHORT_BUFFER
            );
            assert_eq!(header.target_bytes, 0);
            let required = header.message_bytes;
            assert_eq!(
                poll_raw(&runtime, buffer.as_mut_ptr(), required - 1, &mut header),
                POLL_SHORT_BUFFER
            );
            assert_eq!(
                poll_raw(&runtime, buffer.as_mut_ptr(), required, &mut header),
                POLL_RECORD
            );
        }
        assert_eq!(header.level, 2);
        let target = header.target_bytes as usize;
        let end = target + header.message_bytes as usize;
        assert_eq!(&buffer[..target], b"example");
        assert_eq!(
            std::str::from_utf8(&buffer[target..end]).unwrap(),
            "日本語🙂\0end"
        );
        assert!(buffer[end..].iter().all(|byte| *byte == 0xAA));
        assert_eq!(poll(&runtime, &mut [], &mut header), POLL_EMPTY);
        let mut counters = LogStats::default();
        unsafe {
            assert_eq!(stats_raw(&runtime, &mut counters), 0);
        }
        assert_eq!(counters, LogStats::default());
        runtime.shutdown().unwrap();
        assert_eq!(poll(&runtime, &mut buffer, &mut header), POLL_EMPTY);
    }
}
