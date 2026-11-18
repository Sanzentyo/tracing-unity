using System.Collections;
using NUnit.Framework;
using UnityEditor;
using UnityEngine;
using UnityEngine.TestTools;
namespace TracingUnity.Tests
{
    public sealed class PlaySessionTests
    {
        sealed class EmptySource : ILogSource
        {
            bool ready=true; public int Poll(byte[] buffer,out LogHeader header) { header=new LogHeader { Bytes=16, Level=3, MessageBytes=1 }; if(!ready)return 0; ready=false; buffer[0]=65; return 1; }
            public int GetStats(out LogStats stats) { stats=default; return 0; }
        }
        [UnityTest] public IEnumerator RepeatedPlayWithDomainReload() { return Repeat(false); }
        [UnityTest] public IEnumerator RepeatedPlayWithoutDomainReload() { return Repeat(true); }
        static IEnumerator Repeat(bool disableReload)
        {
            var oldEnabled=EditorSettings.enterPlayModeOptionsEnabled;
            var oldOptions=EditorSettings.enterPlayModeOptions;
            EditorSettings.enterPlayModeOptionsEnabled=disableReload;
            EditorSettings.enterPlayModeOptions=EnterPlayModeOptions.DisableDomainReload;
            try
            {
                for(var i=0;i<2;i++)
                {
                    yield return new EnterPlayMode();
                    var go=new GameObject("PlaySessionPump");
                    var pump=go.AddComponent<LogPump>();
                    Assert.IsFalse(pump.IsAttached);
                    pump.WriteToConsole=false; var seen=0; pump.RecordReceived+=r=>seen++; pump.Filter=r=>true;
                    pump.Attach(new EmptySource()); pump.Drain(); Assert.AreEqual(1,seen);
                    yield return null;
                    pump.enabled=false;
                    Assert.IsFalse(pump.IsAttached);
                    Assert.IsNull(pump.Filter);
                    pump.enabled=true;
                    Assert.IsFalse(pump.IsAttached);
                    pump.Attach(new EmptySource());
                    pump.Drain(); Assert.AreEqual(1,seen,"old handler survived disable");
                    Assert.AreEqual(1,go.GetComponents<LogPump>().Length);
                    UnityEngine.Object.Destroy(go);
                    yield return null;
                    Assert.IsFalse(pump.IsAttached);
                    yield return new ExitPlayMode();
                }
            }
            finally
            {
                EditorSettings.enterPlayModeOptionsEnabled=oldEnabled;
                EditorSettings.enterPlayModeOptions=oldOptions;
            }
        }
    }
}
