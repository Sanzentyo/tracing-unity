using System;
using System.Runtime.InteropServices;
namespace TracingUnity
{
    [StructLayout(LayoutKind.Sequential)]
    public struct LogHeader { public uint Bytes, Level, TargetBytes, MessageBytes; }
    [StructLayout(LayoutKind.Sequential)]
    public struct LogStats { public ulong PollDropped, FileDropped, FormatFailed, FileErrors; }
    /// <summary>Caller-owned transport. All methods are called on Unity's main thread.</summary>
    /// <remarks>The owner must keep native code mapped while attached, stop producers and
    /// close/join its native writer explicitly, then detach before unmapping. This interface
    /// does not own or stop a native plugin. Poll uses the documented copied-record ABI.</remarks>
    public interface ILogSource
    {
        int Poll(byte[] buffer, out LogHeader header);
        int GetStats(out LogStats stats);
    }
}
