# Sets or deletes a plugin's secret the way Foxglove stores it (Credential Manager, generic,
# target Foxglove/plugin/<plugin id>/<name>, UTF-8 value), for board checks that need one.
# The value is read from the FOXGLOVE_LAB_SECRET environment variable, never a command-line argument.
#   plugin_secret.ps1 set <plugin id> <name>     |    plugin_secret.ps1 delete <plugin id> <name>
param([Parameter(Mandatory)] [string]$Command, [Parameter(Mandatory)] [string]$Plugin, [Parameter(Mandatory)] [string]$Name)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class LabCred {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  struct CREDENTIAL {
    public int Flags; public int Type; public string TargetName; public string Comment;
    public long LastWritten; public int CredentialBlobSize; public IntPtr CredentialBlob;
    public int Persist; public int AttributeCount; public IntPtr Attributes;
    public string TargetAlias; public string UserName;
  }
  [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  static extern bool CredWriteW(ref CREDENTIAL c, int flags);
  [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  static extern bool CredDeleteW(string target, int type, int flags);
  public static void Set(string target, byte[] value) {
    var c = new CREDENTIAL { Type = 1, TargetName = target, Persist = 2, UserName = "foxglove-plugin",
                             CredentialBlobSize = value.Length, CredentialBlob = Marshal.AllocHGlobal(value.Length) };
    try { Marshal.Copy(value, 0, c.CredentialBlob, value.Length);
          if (!CredWriteW(ref c, 0)) throw new System.ComponentModel.Win32Exception(); }
    finally { Marshal.FreeHGlobal(c.CredentialBlob); }
  }
  public static bool Delete(string target) { return CredDeleteW(target, 1, 0); }
}
'@
$target = "Foxglove/plugin/$Plugin/$Name"
switch ($Command) {
    'set' {
        if (-not $env:FOXGLOVE_LAB_SECRET) { throw 'FOXGLOVE_LAB_SECRET is empty' }
        [LabCred]::Set($target, [Text.Encoding]::UTF8.GetBytes($env:FOXGLOVE_LAB_SECRET))
    }
    'delete' { [void][LabCred]::Delete($target) }
    default { throw 'set or delete' }
}
