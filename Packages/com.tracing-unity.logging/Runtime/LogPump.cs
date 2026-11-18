using System;
using System.Text;
using UnityEngine;
namespace TracingUnity
{
    /// <summary>Instance-owned bounded pump, with no global event handlers or native callbacks.</summary>
    /// <remarks>Attach a fresh source each Play session. OnDisable only detaches; it does not
    /// close native resources. Native lifetime and final flushing belong to the source owner.</remarks>
    [DisallowMultipleComponent]
    public sealed class LogPump : MonoBehaviour
    {
        public const int MaxRecordsPerFrame = 32;
        readonly byte[] buffer = new byte[2176];
        ILogSource source;
        bool draining;
        float nextStats;
        LogStats previous;
        public bool IsAttached => this != null && source != null;
        public void Attach(ILogSource value)
        {
            if (value == null) throw new ArgumentNullException(nameof(value));
            if (this == null || !isActiveAndEnabled) throw new InvalidOperationException("Attach requires a live, active component on the main thread.");
            if (source != null) throw new InvalidOperationException("Detach the current source before attaching another.");
            source = value;
            previous = default;
            nextStats = 0;
        }
        public void Detach() { source = null; }
        void LateUpdate() { Drain(); }
        void OnDisable() { Detach(); }
        void OnDestroy() { Detach(); }
        /// <summary>Drain at most one frame budget. Reentrant calls return immediately.</summary>
        public void Drain()
        {
            if (this == null || !isActiveAndEnabled || source == null || draining) return;
            draining = true;
            var current = source;
            try
            {
                for (var i = 0; i < MaxRecordsPerFrame && ReferenceEquals(source, current); i++)
                {
                    var result = current.Poll(buffer, out var header);
                    if (!ReferenceEquals(source, current) || result == 0 || result == -3) break;
                    var length = (ulong)header.TargetBytes + header.MessageBytes;
                    if (result != 1 || header.Bytes != 16 || header.Level < 1 || header.Level > 5 || length > (ulong)buffer.Length)
                        throw new InvalidOperationException($"Native log ABI rejected: status={result}, bytes={header.Bytes}");
                    var target = Encoding.UTF8.GetString(buffer, 0, checked((int)header.TargetBytes));
                    var message = Encoding.UTF8.GetString(buffer, checked((int)header.TargetBytes), checked((int)header.MessageBytes));
                    var text = $"[Rust/{target}] {message}".Replace("\0", "\\0");
                    try {
                        var type = header.Level == 1 ? LogType.Error : header.Level == 2 ? LogType.Warning : LogType.Log;
                        Debug.LogFormat(type, LogOption.NoStacktrace, null, "{0}", text);
                    } catch { return; } // A host log handler failure does not invalidate the source.
                }
                if (!ReferenceEquals(source, current) || Time.realtimeSinceStartup < nextStats) return;
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
                if (ReferenceEquals(source, current)) Detach();
                Debug.LogError("[Rust/logging] log bridge detached: " + error.Message);
            }
            finally { draining = false; }
        }
    }
}
