using System;
namespace TracingUnity
{
    /// <summary>Original Rust event severity, independent of Unity's three Console categories.</summary>
    public enum LogLevel : uint { Error = 1, Warn = 2, Info = 3, Debug = 4, Trace = 5 }
    public readonly struct LogRecord
    {
        public LogLevel Level { get; }
        public string Target { get; }
        public string Message { get; }
        public LogRecord(LogLevel level, string target, string message)
        { Level = level; Target = target; Message = message; }
    }
    /// <summary>Optional poll-sink control. Never changes file or sibling subscriber filters.</summary>
    public interface ILogLevelControl
    {
        /// <returns>0 applied, -3 busy (retry), -4 closed, -5 unsupported, -1 invalid/error.</returns>
        int SetPollMaxLevel(LogLevel level);
    }
}
