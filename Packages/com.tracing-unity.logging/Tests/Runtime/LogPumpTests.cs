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
        sealed class Source : ILogSource
        {
            public int Remaining, Calls, Status = 1;
            public uint Level = 3;
            public Action DuringPoll;
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
        [Test] public void SeverityPreserved() { var s=new Source{Remaining=1,Level=1}; pump.Attach(s); LogAssert.Expect(LogType.Error,"[Rust/test] hello\\0!"); pump.Drain(); }
    }
}
