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
            public int Poll(byte[] buffer,out LogHeader header) { header=default; return 0; }
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
                    pump.Attach(new EmptySource());
                    yield return null;
                    pump.enabled=false;
                    Assert.IsFalse(pump.IsAttached);
                    pump.enabled=true;
                    Assert.IsFalse(pump.IsAttached);
                    pump.Attach(new EmptySource());
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
