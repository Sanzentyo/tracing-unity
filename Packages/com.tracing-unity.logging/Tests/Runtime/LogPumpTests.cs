using System;
using System.Runtime.InteropServices;
using System.Text;
using NUnit.Framework;
using UnityEngine;
using UnityEngine.TestTools;
namespace TracingUnity.Tests
{
    public sealed class LogPumpTests
    {
        sealed class Source : ILogSource, ILogLevelControl
        {
            public int Remaining, Calls, Status = 1;
            public uint Level = 3;
            public Action DuringPoll; public LogLevel Applied; public int SetStatus, SetCalls; public int SetPollMaxLevel(LogLevel level) { SetCalls++; if(SetStatus==0) Applied=level; return SetStatus; }
            public int Poll(byte[] buffer, out LogHeader header)
            {
                Calls++; DuringPoll?.Invoke();
                header = new LogHeader { Bytes = 16, Level = Level, TargetBytes = 4, MessageBytes = 7 };
                if (Status != 1) return Status;
                if (Remaining-- <= 0) return 0;
                Encoding.UTF8.GetBytes("testhello\0!").CopyTo(buffer, 0);
                return 1;
            }
            public int GetStats(out LogStats stats) { stats = default; return 0; }
        }
        GameObject go;
        LogPump pump;
        [SetUp] public void Setup() { go = new GameObject("LogPumpTest"); pump = go.AddComponent<LogPump>(); }
        [TearDown] public void Teardown() { UnityEngine.Object.DestroyImmediate(go); }
        [Test] public void InactiveAttachRejected() { go.SetActive(false); Assert.Throws<InvalidOperationException>(()=>pump.Attach(new Source())); }
        [Test] public void DestroyedPumpCannotPoll() { var s=new Source{Remaining=5}; pump.Attach(s); UnityEngine.Object.DestroyImmediate(go); pump.Drain(); Assert.AreEqual(0,s.Calls); Assert.IsFalse(pump.IsAttached); }
        [Test] public void AbiMatches() { Assert.AreEqual(16, Marshal.SizeOf<LogHeader>()); Assert.AreEqual(12, Marshal.OffsetOf<LogHeader>(nameof(LogHeader.MessageBytes)).ToInt32()); Assert.AreEqual(32, Marshal.SizeOf<LogStats>()); }
        [Test] public void BudgetAndContinuation() { var s=new Source{Remaining=40}; pump.Attach(s); pump.Drain(); Assert.AreEqual(32,s.Calls); pump.Drain(); Assert.AreEqual(41,s.Calls); }
        [Test] public void ReentrantDrainReturns() { var s=new Source{Remaining=2}; s.DuringPoll=pump.Drain; pump.Attach(s); pump.Drain(); Assert.AreEqual(3,s.Calls); }
        [Test] public void DuplicateAttachRejected() { pump.Attach(new Source()); Assert.Throws<InvalidOperationException>(()=>pump.Attach(new Source())); }
        [Test] public void DetachDuringPollDoesNotPollAgain() { var s=new Source{Remaining=100}; s.DuringPoll=pump.Detach; pump.Attach(s); pump.Drain(); Assert.AreEqual(1,s.Calls); Assert.IsFalse(pump.IsAttached); }
        [Test] public void DetachReattachHasNoStaleSource() { var old=new Source{Remaining=3}; pump.Attach(old); pump.Detach(); pump.Drain(); Assert.AreEqual(0,old.Calls); var fresh=new Source{Remaining=1}; pump.Attach(fresh); pump.Drain(); Assert.AreEqual(2,fresh.Calls); }
        [TestCase(-1)] [TestCase(-2)] public void BadStatusDetachesOnce(int status) { var s=new Source{Status=status}; pump.Attach(s); LogAssert.Expect(LogType.Error,$"[Rust/logging] log bridge detached: Native log ABI rejected: status={status}, bytes=16"); pump.Drain(); pump.Drain(); Assert.AreEqual(1,s.Calls); Assert.IsFalse(pump.IsAttached); }
        [Test] public void BusyKeepsAttachment() { var s=new Source{Status=-3}; pump.Attach(s); pump.Drain(); Assert.IsTrue(pump.IsAttached); Assert.AreEqual(1,s.Calls); }
        [Test] public void ExceptionDetachesOnce() { var s=new Source{DuringPoll=()=>throw new Exception("test failure")}; pump.Attach(s); LogAssert.Expect(LogType.Error,"[Rust/logging] log bridge detached: test failure"); pump.Drain(); pump.Drain(); Assert.AreEqual(1,s.Calls); }
        [TestCase(LogLevel.Error, LogType.Error)] [TestCase(LogLevel.Warn, LogType.Warning)]
        [TestCase(LogLevel.Info, LogType.Log)] [TestCase(LogLevel.Debug, LogType.Log)] [TestCase(LogLevel.Trace, LogType.Log)]
        public void SeverityPreserved(LogLevel level, LogType type)
        {
            var s=new Source{Remaining=1,Level=(uint)level}; pump.Attach(s);
            var thread=System.Threading.Thread.CurrentThread.ManagedThreadId; LogRecord? delivered=null;
            pump.RecordReceived += r => { Assert.AreEqual(thread,System.Threading.Thread.CurrentThread.ManagedThreadId); delivered=r; };
            LogAssert.Expect(type,$"[Rust/{level.ToString().ToUpperInvariant()}/test] hello\\0!"); pump.Drain();
            Assert.IsTrue(delivered.HasValue); Assert.AreEqual(level,delivered.Value.Level); Assert.AreEqual("hello\0!",delivered.Value.Message);
        }
        [Test] public void NativeThresholdChangesAndBusyRetries()
        {
            var s=new Source(); pump.Attach(s); pump.Drain(); Assert.AreEqual(LogLevel.Info,s.Applied);
            pump.NativeMaxLevel=LogLevel.Debug; s.SetStatus=-3; pump.Drain(); Assert.AreEqual(LogLevel.Info,s.Applied);
            s.SetStatus=0; Assert.AreEqual(0,pump.ApplyNativeMaxLevel()); Assert.AreEqual(LogLevel.Debug,s.Applied);
            pump.NativeMaxLevel=LogLevel.Info; pump.Drain(); Assert.AreEqual(LogLevel.Info,s.Applied);
            pump.NativeMaxLevel=LogLevel.Trace; pump.Drain(); Assert.AreEqual(LogLevel.Trace,s.Applied);
            Assert.Throws<ArgumentOutOfRangeException>(()=>pump.NativeMaxLevel=(LogLevel)0);
        }
        [Test] public void ManagedFilterAndCallbackExceptionsDoNotDetach()
        {
            var s=new Source{Remaining=1,Level=4}; pump.Attach(s); pump.WriteToConsole=false; var received=0;
            pump.RecordReceived+=r=>throw new Exception("handler"); pump.RecordReceived+=r=>received++;
            pump.DeliveryMaxLevel=LogLevel.Info; pump.Drain(); Assert.AreEqual(0,received);
            s.Remaining=1; pump.DeliveryMaxLevel=LogLevel.Trace; pump.Filter=r=>false; pump.Drain(); Assert.AreEqual(0,received);
            s.Remaining=1; pump.Filter=r=>throw new Exception("filter"); pump.Drain(); Assert.AreEqual(0,received);
            s.Remaining=1; pump.Filter=null; pump.Drain(); Assert.AreEqual(1,received); Assert.IsTrue(pump.IsAttached);
            Assert.AreEqual(LogLevel.Info,s.Applied);
        }
        [Test] public void HandlerDetachSuppressesRemainingHandlersAndConsole()
        {
            var s=new Source{Remaining=2}; pump.Attach(s); var second=0;
            pump.RecordReceived+=r=>{pump.Drain();pump.Detach();}; pump.RecordReceived+=r=>second++;
            pump.Drain(); Assert.AreEqual(0,second); Assert.AreEqual(1,s.Calls);
        }
        [Test] public void SameSourceReattachStopsOldDelivery()
        {
            var s=new Source{Remaining=2}; pump.Attach(s); var first=0; var second=0;
            pump.RecordReceived+=r=>{first++;pump.Detach();pump.Attach(s);}; pump.RecordReceived+=r=>second++;
            pump.Drain(); Assert.AreEqual(1,first); Assert.AreEqual(0,second); Assert.AreEqual(1,s.Calls);
        }
        [Test] public void UnsupportedControlFallsBackWithoutRetryingEveryFrame()
        {
            var s=new Source{Remaining=2,SetStatus=-5}; pump.WriteToConsole=false; pump.Attach(s);
            var seen=0;pump.RecordReceived+=r=>seen++;pump.Drain();pump.Drain();
            Assert.AreEqual(2,seen);Assert.AreEqual(1,s.SetCalls);Assert.IsFalse(pump.SupportsNativeLevelControl);Assert.IsTrue(pump.IsAttached);
            Assert.AreEqual(-5,pump.ApplyNativeMaxLevel());Assert.AreEqual(1,s.SetCalls);
        }        [TestCase(-4)] [TestCase(-3)] public void ClosedOrBusyControlStillDrains(int status)
        {
            var s=new Source{Remaining=3,SetStatus=status};pump.Attach(s);pump.WriteToConsole=false;var seen=0;pump.RecordReceived+=r=>seen++;
            pump.Drain();pump.Drain();Assert.AreEqual(3,seen);Assert.IsTrue(pump.IsAttached);Assert.AreEqual(status==-4?1:2,s.SetCalls);
        }
        [Test] public void ExplicitApplyRepairsExternalLevelChange()
        {
            var s=new Source();pump.Attach(s);pump.Drain();s.Applied=LogLevel.Trace;
            Assert.AreEqual(0,pump.ApplyNativeMaxLevel());Assert.AreEqual(LogLevel.Info,s.Applied);
            s.Applied=LogLevel.Debug;pump.InvalidateNativeLevel();pump.Drain();Assert.AreEqual(LogLevel.Info,s.Applied);
            pump.Detach();var other=new Source();pump.Attach(other);pump.Drain();Assert.AreEqual(1,other.SetCalls);
        }
        [Test] public void InvalidControlDetaches()
        {
            var s=new Source{SetStatus=-1};pump.Attach(s);LogAssert.Expect(LogType.Error,"[Rust/logging] log bridge detached: Native log level rejected: status=-1");pump.Drain();Assert.IsFalse(pump.IsAttached);Assert.AreEqual(0,s.Calls);
        }
        [Test] public void SerializedInspectorChangeAppliesBeforePolling()
        {
            var s=new Source();pump.Attach(s);pump.Drain();
            var serialized=new UnityEditor.SerializedObject(pump);serialized.FindProperty("nativeMaxLevel").intValue=(int)LogLevel.Trace;serialized.ApplyModifiedPropertiesWithoutUndo();
            pump.Drain();Assert.AreEqual(LogLevel.Trace,s.Applied);
        }
    }
}
