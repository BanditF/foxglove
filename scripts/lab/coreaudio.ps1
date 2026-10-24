# Reads or sets the mute and volume of Windows' default capture endpoints, straight through Core
# Audio (not through Foxglove), so the lab can check Foxglove's mic broker and put things back.
#   coreaudio.ps1 get                         -> JSON [{role, id, name, muted, volume}] (console, communications)
#   coreaudio.ps1 set <role> <muted> <volume>  role = console|communications, muted = 0|1, volume = 0..1
param([Parameter(Mandatory)] [string]$Command, [string]$Role, [string]$Muted, [string]$Volume)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace LabAudio {
  [ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")] class MMDeviceEnumerator {}
  [ComImport, Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IMMDeviceEnumerator {
    int NotImpl1();
    [PreserveSig] int GetDefaultAudioEndpoint(int dataFlow, int role, out IMMDevice device);
  }
  [ComImport, Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IMMDevice {
    [PreserveSig] int Activate(ref Guid iid, int clsCtx, IntPtr p, [MarshalAs(UnmanagedType.IUnknown)] out object o);
    [PreserveSig] int OpenPropertyStore(int access, out IPropertyStore store);
    [PreserveSig] int GetId([MarshalAs(UnmanagedType.LPWStr)] out string id);
  }
  [ComImport, Guid("886d8eeb-8cf2-4446-8d02-cdba1dbdcf99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IPropertyStore {
    int GetCount(out int c); int GetAt(int i, out PropertyKey k);
    [PreserveSig] int GetValue(ref PropertyKey k, out PropVariant v);
  }
  [StructLayout(LayoutKind.Sequential)] struct PropertyKey { public Guid fmtid; public int pid; }
  [StructLayout(LayoutKind.Explicit)] struct PropVariant { [FieldOffset(0)] public short vt; [FieldOffset(8)] public IntPtr p; }
  [ComImport, Guid("5CDF2C82-841E-4546-9722-0CF74078229A"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IAudioEndpointVolume {
    int RegisterControlChangeNotify(IntPtr p); int UnregisterControlChangeNotify(IntPtr p);
    int GetChannelCount(out int c); int SetMasterVolumeLevel(float l, ref Guid g);
    int SetMasterVolumeLevelScalar(float l, ref Guid g); int GetMasterVolumeLevel(out float l);
    int GetMasterVolumeLevelScalar(out float l); int SetChannelVolumeLevel(int c, float l, ref Guid g);
    int SetChannelVolumeLevelScalar(int c, float l, ref Guid g); int GetChannelVolumeLevel(int c, out float l);
    int GetChannelVolumeLevelScalar(int c, out float l);
    int SetMute([MarshalAs(UnmanagedType.Bool)] bool m, ref Guid g); int GetMute([MarshalAs(UnmanagedType.Bool)] out bool m);
  }
  public class Endpoint {
    public string Id, Name; public bool Muted; public float Volume;
    static IAudioEndpointVolume Open(int role, out IMMDevice dev) {
      var e = (IMMDeviceEnumerator)new MMDeviceEnumerator();
      Marshal.ThrowExceptionForHR(e.GetDefaultAudioEndpoint(1, role, out dev)); // 1 = eCapture
      var iid = typeof(IAudioEndpointVolume).GUID; object o;
      Marshal.ThrowExceptionForHR(dev.Activate(ref iid, 23, IntPtr.Zero, out o));
      return (IAudioEndpointVolume)o;
    }
    public static Endpoint Get(int role) {
      IMMDevice dev; var v = Open(role, out dev); var r = new Endpoint();
      dev.GetId(out r.Id); v.GetMute(out r.Muted); v.GetMasterVolumeLevelScalar(out r.Volume);
      IPropertyStore s; dev.OpenPropertyStore(0, out s);
      var k = new PropertyKey { fmtid = new Guid("a45c254e-df1c-4efd-8020-67d146a850e0"), pid = 14 }; PropVariant pv;
      if (s.GetValue(ref k, out pv) == 0 && pv.vt == 31) r.Name = Marshal.PtrToStringUni(pv.p);
      return r;
    }
    public static void Set(int role, bool muted, float volume) {
      IMMDevice dev; var v = Open(role, out dev); var g = Guid.Empty;
      v.SetMasterVolumeLevelScalar(volume, ref g); v.SetMute(muted, ref g);
    }
  }
}
'@
$roles = @{ console = 0; communications = 2 }
switch ($Command) {
    'get' {
        $out = foreach ($name in 'console', 'communications') {
            try {
                $e = [LabAudio.Endpoint]::Get($roles[$name])
                [pscustomobject]@{ role = $name; id = $e.Id; name = $e.Name; muted = $e.Muted; volume = [math]::Round($e.Volume, 4) }
            } catch { [pscustomobject]@{ role = $name; id = $null; error = $_.Exception.Message } }
        }
        ConvertTo-Json -Compress -InputObject @($out)
    }
    'set' { [LabAudio.Endpoint]::Set($roles[$Role], $Muted -eq '1', [float]$Volume) }
    default { throw "get or set" }
}
