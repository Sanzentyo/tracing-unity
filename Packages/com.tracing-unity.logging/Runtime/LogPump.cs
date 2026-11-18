using System;
using System.Text;
using UnityEngine;
namespace TracingUnity
{
    /// <summary>Instance-owned, bounded main-thread delivery. No global handlers or native callbacks.</summary>
    [DisallowMultipleComponent]
    public sealed class LogPump : MonoBehaviour
    {
        public const int MaxRecordsPerFrame = 32;
        [SerializeField, Tooltip("Rust poll queue threshold. Requires ILogLevelControl; changes apply on the main thread before the next poll.")]
        LogLevel nativeMaxLevel = LogLevel.Info;
        [SerializeField, Tooltip("Managed delivery threshold, independent of native file output.")]
        LogLevel deliveryMaxLevel = LogLevel.Trace;
        [SerializeField] bool writeToConsole = true;
        readonly byte[] buffer = new byte[2176];
        ILogSource source;
        ulong generation;
        LogLevel? appliedNativeLevel;
        bool draining;
        bool nativeControlUnavailable;
        bool nativeControlClosed;
        float nextStats;
        LogStats previous;
        /// <summary>Called on the main thread after managed filtering, with the original level and text.</summary>
        public event Action<LogRecord> RecordReceived;
        /// <summary>Optional main-thread filter, applied before handlers and Console. Exceptions skip that record.</summary>
        public Func<LogRecord, bool> Filter { get; set; }
        public bool WriteToConsole { get => writeToConsole; set => writeToConsole = value; }
        public LogLevel DeliveryMaxLevel { get => deliveryMaxLevel; set { Validate(value); deliveryMaxLevel = value; } }
        /// <summary>Requested native threshold. Call ApplyNativeMaxLevel to apply synchronously;
        /// otherwise it is applied before the next Drain. Existing queued records are not erased.</summary>
        public LogLevel NativeMaxLevel { get => nativeMaxLevel; set { Validate(value); nativeMaxLevel = value; } }
        public bool SupportsNativeLevelControl => source is ILogLevelControl && !nativeControlUnavailable;
        public bool IsAttached => this != null && source != null;
        static void Validate(LogLevel level)
        { if (level < LogLevel.Error || level > LogLevel.Trace) throw new ArgumentOutOfRangeException(nameof(level)); }
        public void Attach(ILogSource value)
        {
            if (value == null) throw new ArgumentNullException(nameof(value));
            if (this == null || !isActiveAndEnabled) throw new InvalidOperationException("Attach requires a live, active component on the main thread.");
            if (source != null) throw new InvalidOperationException("Detach the current source before attaching another.");
            generation++; source = value; appliedNativeLevel = null; nativeControlUnavailable = false; nativeControlClosed = false; previous = default; nextStats = 0;
        }
        /// <summary>0 applied; -3 busy; -4 inactive/unattached/closed; -5 unsupported; -1 invalid/error.
        /// All calls require the main thread. A busy update is retried before the next poll.</summary>
        public int ApplyNativeMaxLevel()
        {
            if (this == null || !isActiveAndEnabled || source == null) return -4;
            if (nativeMaxLevel < LogLevel.Error || nativeMaxLevel > LogLevel.Trace) return -1;
            if (nativeControlUnavailable || !(source is ILogLevelControl control)) return -5;
            if (nativeControlClosed) return -4;
            var current = source;
            var currentGeneration = generation;
            var requested = nativeMaxLevel;
            var result = control.SetPollMaxLevel(requested);
            if (!ReferenceEquals(source, current) || generation != currentGeneration) return -4;
            if (result == -4) nativeControlClosed = true;
            if (result == -5 && (ReferenceEquals(source, current) && generation == currentGeneration)) nativeControlUnavailable = true;
            if (result == 0 && (ReferenceEquals(source, current) && generation == currentGeneration)) appliedNativeLevel = requested;
            return result;
        }
        /// <summary>Call after host-side filter changes when the requested Inspector value should be reapplied.</summary>
        public void InvalidateNativeLevel() { appliedNativeLevel = null; }
        public void Detach() { generation++; source = null; appliedNativeLevel = null; }
        void LateUpdate() { Drain(); }
        void OnDisable() { Detach(); RecordReceived = null; Filter = null; }
        void OnDestroy() { Detach(); RecordReceived = null; Filter = null; }
        /// <summary>Drain at most one frame budget. Reentrant calls return immediately.</summary>
        public void Drain()
        {
            if (this == null || !isActiveAndEnabled || source == null || draining) return;
            draining = true;
            var current = source;
            var currentGeneration = generation;
            try
            {
                Validate(deliveryMaxLevel);
                if (current is ILogLevelControl && !nativeControlUnavailable && !nativeControlClosed && appliedNativeLevel != nativeMaxLevel)
                {
                    var status = ApplyNativeMaxLevel();
                    if (!(ReferenceEquals(source, current) && generation == currentGeneration) ) return;
                    if (status != 0 && status != -3 && status != -4 && status != -5) throw new InvalidOperationException($"Native log level rejected: status={status}");
                }
                for (var i = 0; i < MaxRecordsPerFrame && (ReferenceEquals(source, current) && generation == currentGeneration); i++)
                {
                    var result = current.Poll(buffer, out var header);
                    if (!(ReferenceEquals(source, current) && generation == currentGeneration) || result == 0 || result == -3) break;
                    var length = (ulong)header.TargetBytes + header.MessageBytes;
                    if (result != 1 || header.Bytes != 16 || header.Level < 1 || header.Level > 5 || length > (ulong)buffer.Length)
                        throw new InvalidOperationException($"Native log ABI rejected: status={result}, bytes={header.Bytes}");
                    var record = new LogRecord((LogLevel)header.Level,
                        Encoding.UTF8.GetString(buffer, 0, checked((int)header.TargetBytes)),
                        Encoding.UTF8.GetString(buffer, checked((int)header.TargetBytes), checked((int)header.MessageBytes)));
                    if (record.Level > deliveryMaxLevel) continue;
                    try { if (Filter != null && !Filter(record)) continue; }
                    catch { continue; } // User callbacks never invalidate the native source.
                    if (!(ReferenceEquals(source, current) && generation == currentGeneration)) break;
                    var handlers = RecordReceived;
                    if (handlers != null)
                        foreach (Action<LogRecord> handler in handlers.GetInvocationList())
                        {
                            if (!(ReferenceEquals(source, current) && generation == currentGeneration)) break;
                            try { handler(record); } catch { /* Isolate individual handlers. */ }
                        }
                    if (!(ReferenceEquals(source, current) && generation == currentGeneration)) break;
                    if (writeToConsole)
                    {
                        var type = record.Level == LogLevel.Error ? LogType.Error : record.Level == LogLevel.Warn ? LogType.Warning : LogType.Log;
                        var text = $"[Rust/{record.Level.ToString().ToUpperInvariant()}/{record.Target}] {record.Message}".Replace("\0", "\\0");
                        try { Debug.LogFormat(type, LogOption.NoStacktrace, null, "{0}", text); }
                        catch { return; }
                    }
                }
                if (!(ReferenceEquals(source, current) && generation == currentGeneration) || Time.realtimeSinceStartup < nextStats) return;
                nextStats = Time.realtimeSinceStartup + 5;
                if (current.GetStats(out var stats) == 0)
                {
                    if (stats.PollDropped != previous.PollDropped || stats.FileDropped != previous.FileDropped ||
                        stats.FormatFailed != previous.FormatFailed || stats.FileErrors != previous.FileErrors)
                        try { Debug.LogFormat(LogType.Warning, LogOption.NoStacktrace, null, "[Rust/logging] cumulative pollDropped={0} fileDropped={1} formatFailed={2} fileErrors={3}", stats.PollDropped, stats.FileDropped, stats.FormatFailed, stats.FileErrors); } catch { return; }
                    previous = stats;
                }
            }
            catch (Exception error)
            {
                if ((ReferenceEquals(source, current) && generation == currentGeneration)) Detach();
                Debug.LogError("[Rust/logging] log bridge detached: " + error.Message);
            }
            finally { draining = false; }
        }
    }
}
