# Dumps the UI Automation tree of an ALREADY RUNNING Foxglove window (#1376), as JSON, and checks
# that expected nodes are in it. ATTACH ONLY: this script never starts Foxglove (or any program);
# it reads the process you name by id. Windows PowerShell 5.1, UIAutomationClient.
#
#   uia_tree.ps1 -ProcessId 1234 [-Title Foxglove] [-Page Scenes] [-Expect 'Button:Start','RadioButton:Temperature:on']
#                [-TimeoutSec 20] [-MaxNodes 4000] [-OutFile tree.json]
#   uia_tree.ps1 -SelfTest        (pure parts only: JSON shape, expectation matching; no UIA, no process)
#   uia_tree.ps1 -LiveProbe       (can this PC read LiveSetting? one read on the desktop root; no Foxglove, no window)
#
# Needs FOXGLOVE_APP_OK=1 (On-call's OK, like every lab case that touches Foxglove): the first read
# of the tree turns AccessKit on in that process until it restarts, which changes its CPU profile.
# Point it at a lab instance (isolated FOXGLOVE_CONFIG_DIR) unless On-call says otherwise.
#
# The process must be named exactly `foxglove`, or `hideout` with -ProcessName hideout (never a wildcard; a lab hub's window is titled "Hideout (lab)", pass it as -Title). Window: the top-level window of
# that process whose name equals -Title (default "Foxglove", the Settings window; the overlay's
# window is named "Foxglove overlay").
#
# -Expect items are Role:Name or Role:Name:state. Role is a UIA control type (Button, RadioButton,
# Group, Text, ...), Name the exact accessible name, state `on` (selected or toggled on) or `off`.
#
# -Page <name> (Keyboard, Lights, Scenes, Screens, Plugins, App) opens that page first, through
# UI Automation only: the sidebar entry of that name is selected, invoked or (toggled buttons, only when off) toggled with its UIA pattern
# (no mouse, no keys, no focus change), then the tree is read after a short settle. It changes the
# page the Settings window shows, as a click would, so use a lab instance. Nothing else is acted on.
#
# The process must belong to you (same user and Windows session). If several windows of it carry
# the title (an older build titled the overlay "Foxglove" too) the script refuses and lists them; pick one with
# -Handle (decimal or 0x hex native window handle from that list).
#
# LiveSetting (the live region, UIA property 30135) is read through COM UI Automation
# (CUIAutomation, IUIAutomationElement::GetCurrentPropertyValue): the managed client this script
# walks with does not know that property. The COM read lists the window's elements once, and each
# dumped node gets its value by its UIA runtime id. liveSetting is 0, 1 or 2 when read, and null
# when not (COM not available, or a node the COM list did not have): never a guess.
#
# Exit codes: 0 all expectations met (or none given), 2 an expectation is missing, 3 refused or
# window not found or ambiguous or the -Page entry not found, 4 timed out (a hard limit: a hung window cannot block it). Nothing is clicked, typed or focused, and the tree is only read, except that -Page selects, invokes or toggles one sidebar entry through UIA.
param(
    [int]$ProcessId = 0,
    [string]$Title = 'Foxglove',
    [ValidateSet('foxglove', 'hideout')][string]$ProcessName = 'foxglove',
    [string[]]$Expect = @(),
    [int]$TimeoutSec = 20,
    [int]$MaxNodes = 4000,
    [string]$OutFile = '',
    [string]$Handle = '',
    [string]$Page = '',
    [switch]$SelfTest,
    [switch]$LiveProbe
)
$ErrorActionPreference = 'Stop'
# Names carry characters outside ASCII ("Update firmware…", "0.3.0 (dev) · 2d6a7fe"): write them as
# UTF-8, or a console on a legacy code page shows them as "â€¦" and "Â·".
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false) } catch { }
$OutputEncoding = New-Object System.Text.UTF8Encoding($false)

# ---- pure parts (tested by -SelfTest) --------------------------------------------------------

# One dumped node as a plain object: the shape of the JSON.
# liveSetting: UIA's LiveSetting as a number (0 off, 1 polite, 2 assertive), or $null when the
# client cannot read that property (the tree then has no live information at all).
function New-NodeRecord($role, $name, $autoId, $enabled, $state, $rect, $children, $live = $null) {
    [ordered]@{
        role = $role; name = $name; automationId = $autoId; enabled = $enabled
        state = $state; rect = $rect; liveSetting = $live; children = @($children)
    }
}

# The COM read of LiveSetting. Only the methods called are named; the others hold their place,
# because a COM interface is called by position: IUIAutomation is CompareElements,
# CompareRuntimeIds, GetRootElement, ElementFromHandle, ... CreateTrueCondition (19th);
# IUIAutomationElement is SetFocus, GetRuntimeId, FindFirst, FindAll, ... GetCurrentPropertyValue
# (8th). The order is uiautomationclient.h's. Nothing here sets, invokes or focuses anything.
$LiveSource = @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace FoxgloveLab {
    [ComImport, Guid("ff48dba4-60ef-4201-aa87-54103eef594e")]
    class CUIAutomation { }

    [ComImport, Guid("352ffba8-0973-437c-a61f-f64cafd81df9"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IUIAutomationCondition { }

    [ComImport, Guid("14314595-b4bc-4055-95f2-58f2e42c9855"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IUIAutomationElementArray {
        [PreserveSig] int get_Length(out int length);
        [PreserveSig] int GetElement(int index, out IUIAutomationElement element);
    }

    [ComImport, Guid("d22108aa-8ac5-49a5-837b-37bbb3d7591e"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IUIAutomationElement {
        void _SetFocus();
        [PreserveSig] int GetRuntimeId([MarshalAs(UnmanagedType.SafeArray, SafeArraySubType = VarEnum.VT_I4)] out int[] runtimeId);
        void _FindFirst();
        [PreserveSig] int FindAll(int scope, IUIAutomationCondition condition, out IUIAutomationElementArray found);
        void _FindFirstBuildCache();
        void _FindAllBuildCache();
        void _BuildUpdatedCache();
        [PreserveSig] int GetCurrentPropertyValue(int propertyId, [MarshalAs(UnmanagedType.Struct)] out object value);
    }

    [ComImport, Guid("30cbe57d-d9d0-452a-ab13-7ac5ac4825ee"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IUIAutomation {
        void _CompareElements();
        void _CompareRuntimeIds();
        [PreserveSig] int GetRootElement(out IUIAutomationElement root);
        [PreserveSig] int ElementFromHandle(IntPtr hwnd, out IUIAutomationElement element);
        void _ElementFromPoint();
        void _GetFocusedElement();
        void _GetRootElementBuildCache();
        void _ElementFromHandleBuildCache();
        void _ElementFromPointBuildCache();
        void _GetFocusedElementBuildCache();
        void _CreateTreeWalker();
        void _get_ControlViewWalker();
        void _get_ContentViewWalker();
        void _get_RawViewWalker();
        void _get_RawViewCondition();
        void _get_ControlViewCondition();
        void _get_ContentViewCondition();
        void _CreateCacheRequest();
        [PreserveSig] int CreateTrueCondition(out IUIAutomationCondition condition);
    }

    public static class UiaLive {
        public const int LiveSettingProperty = 30135;  // UIA_LiveSettingPropertyId
        const int Subtree = 7;                          // TreeScope_Subtree

        // A runtime id as text: what the managed and the COM client both give for one element.
        public static string Key(int[] runtimeId) {
            return runtimeId == null ? "" : string.Join(".", Array.ConvertAll(runtimeId, delegate(int i) { return i.ToString(); }));
        }

        static void Ok(int hr, string what) {
            if (hr < 0) throw new InvalidOperationException(what + " failed: 0x" + hr.ToString("X8"));
        }

        static object Live(IUIAutomationElement e) {
            object v;
            if (e.GetCurrentPropertyValue(LiveSettingProperty, out v) < 0) return null;
            return v is int ? v : null;
        }

        // LiveSetting of the desktop root element (0 there): proves the COM read works on this PC.
        public static object Root() {
            IUIAutomation uia = (IUIAutomation)new CUIAutomation();
            IUIAutomationElement root;
            Ok(uia.GetRootElement(out root), "GetRootElement");
            if (root == null) throw new InvalidOperationException("no root element");
            return Live(root);
        }

        // Runtime id -> LiveSetting for the window and everything under it (at most `max`
        // elements, none after `deadlineTicks`, UTC). An element whose value did not read is left out.
        public static Dictionary<string, int> Map(IntPtr hwnd, int max, long deadlineTicks) {
            IUIAutomation uia = (IUIAutomation)new CUIAutomation();
            IUIAutomationElement window;
            Ok(uia.ElementFromHandle(hwnd, out window), "ElementFromHandle");
            if (window == null) throw new InvalidOperationException("no element for the window");
            IUIAutomationCondition all;
            Ok(uia.CreateTrueCondition(out all), "CreateTrueCondition");
            IUIAutomationElementArray found;
            Ok(window.FindAll(Subtree, all, out found), "FindAll");
            Dictionary<string, int> map = new Dictionary<string, int>();
            if (found == null) return map;
            int n;
            Ok(found.get_Length(out n), "get_Length");
            for (int i = 0; i < n && i < max && DateTime.UtcNow.Ticks < deadlineTicks; i++) {
                IUIAutomationElement e;
                int[] id;
                if (found.GetElement(i, out e) < 0 || e == null) continue;
                if (e.GetRuntimeId(out id) < 0 || id == null || id.Length == 0) continue;
                object v = Live(e);
                if (v != null) map[Key(id)] = (int)v;
            }
            return map;
        }
    }
}
'@

# "Button:Start" -> @{ Role; Name; State }. State is $null when not given.
function ConvertTo-Expectation([string]$text) {
    $parts = $text.Split(':')
    if ($parts.Count -lt 2 -or $parts.Count -gt 3 -or -not $parts[0] -or -not $parts[1]) {
        throw "bad -Expect '$text' (want Role:Name or Role:Name:on|off)"
    }
    $state = $null
    if ($parts.Count -eq 3) {
        if ($parts[2] -notin 'on', 'off') { throw "bad state in -Expect '$text' (on or off)" }
        $state = $parts[2]
    }
    [pscustomobject]@{ Role = $parts[0]; Name = $parts[1]; State = $state }
}

# The pages the sidebar lists, by name (the names the app gives them).
$PageNames = 'Keyboard', 'Lights', 'Scenes', 'Screens', 'Plugins', 'App'
function Test-PageName([string]$name) { $PageNames -ccontains $name }

# Every node of a record tree, parents first.
function Get-AllNodes($node) {
    $node
    foreach ($c in $node.children) { Get-AllNodes $c }
}

# The expectations the tree does not meet (empty = all met). A state of on/off compares the node's
# state field ('on' when selected or toggled on, 'off' when selectable and not, else $null).
function Find-Missing($tree, [string[]]$expect) {
    $nodes = @(Get-AllNodes $tree)
    foreach ($e in $expect) {
        $x = ConvertTo-Expectation $e
        $hit = $nodes | Where-Object {
            $_.role -ceq $x.Role -and $_.name -ceq $x.Name -and ($null -eq $x.State -or $_.state -ceq $x.State)
        } | Select-Object -First 1
        if (-not $hit) { $e }
    }
}

if ($SelfTest) {
    $fail = @()
    function Check($ok, $what) { if (-not $ok) { $script:fail += $what } }
    $rect = [ordered]@{ x = 0; y = 0; w = 10; h = 10 }
    $tree = New-NodeRecord 'Window' 'Foxglove' '' $true $null $rect @(
        (New-NodeRecord 'Group' 'Effect' '' $true $null $rect @(
            (New-NodeRecord 'RadioButton' 'Temperature' '' $true 'on' $rect @()),
            (New-NodeRecord 'RadioButton' 'Breathe' '' $true 'off' $rect @()))),
        (New-NodeRecord 'Button' 'Start' '' $true $null $rect @()))
    Check ((@(Find-Missing $tree @('Button:Start', 'Group:Effect', 'RadioButton:Temperature:on'))).Count -eq 0) 'present nodes are found'
    $missing = @(Find-Missing $tree @('Button:Stop', 'RadioButton:Breathe:on', 'button:Start'))
    Check ($missing.Count -eq 3) "missing nodes are reported (got $($missing.Count))"
    Check ($missing[0] -eq 'Button:Stop') 'the missing name is echoed'
    $x = ConvertTo-Expectation 'RadioButton:Temperature:on'
    Check ($x.Role -eq 'RadioButton' -and $x.Name -eq 'Temperature' -and $x.State -eq 'on') 'expectation parses'
    Check ($null -eq (ConvertTo-Expectation 'Button:Start').State) 'state is optional'
    foreach ($bad in 'Start', 'A:B:maybe', ':x', 'A::') {
        $threw = $false
        try { ConvertTo-Expectation $bad | Out-Null } catch { $threw = $true }
        Check $threw "bad expectation '$bad' is refused"
    }
    Check ((Test-PageName 'Scenes') -and -not (Test-PageName 'scenes') -and -not (Test-PageName 'Settings')) 'page names are exact'
    $odd = "Update firmware$([char]0x2026) 0.3.0 (dev) $([char]0xB7) 2d6a7fe"
    $rt = ((New-NodeRecord 'Button' $odd '' $true $null $rect @()) | ConvertTo-Json | ConvertFrom-Json).name
    Check ($rt -ceq $odd) 'non-ASCII names survive JSON'
    Check ([Console]::OutputEncoding.CodePage -eq 65001 -or $Host.Name -ne 'ConsoleHost') 'output is UTF-8'
    $json = $tree | ConvertTo-Json -Depth 20
    $back = $json | ConvertFrom-Json
    Check ($back.children[1].name -eq 'Start' -and $back.children[0].children.Count -eq 2) 'JSON keeps the shape'
    Check ($null -eq $back.liveSetting -and $back.PSObject.Properties.Name -contains 'liveSetting') 'liveSetting is in every record, null when not read'
    Check ((New-NodeRecord 'Text' '' '' $true $null $rect @() 1).liveSetting -eq 1) 'liveSetting carries the number'
    Check ((New-NodeRecord 'Text' '' '' $true $null $rect @() 0 | ConvertTo-Json | ConvertFrom-Json).liveSetting -eq 0) 'a read 0 (off) is 0, not null'
    # The COM read compiles here (Windows PowerShell's own C# compiler); nothing of it runs but Key.
    $compiled = $false
    try { Add-Type -TypeDefinition $LiveSource; $compiled = $true } catch { $script:fail += "the COM LiveSetting reader does not compile: $_" }
    if ($compiled) {
        Check ([FoxgloveLab.UiaLive]::Key([int[]](42, 1234, 7)) -ceq '42.1234.7') 'a runtime id is keyed by its numbers'
        Check ([FoxgloveLab.UiaLive]::Key($null) -ceq '') 'no runtime id is no key'
        Check ([FoxgloveLab.UiaLive]::LiveSettingProperty -eq 30135) 'the property is UIA_LiveSettingPropertyId'
    }
    # Attach only: the script text starts nothing.
    $self = Get-Content -LiteralPath $PSCommandPath -Raw
    Check (-not $self.Contains('Lookup' + 'ById')) 'LiveSetting is not asked of the managed client (it does not know it)'
    # A COM interface is called by position: the named methods stay in the header's order.
    $at = @('void _SetFocus();', 'int GetRuntimeId(', 'void _FindFirst();', 'int FindAll(', 'void _BuildUpdatedCache();', 'int GetCurrentPropertyValue(',
            'void _CompareRuntimeIds();', 'int GetRootElement(', 'int ElementFromHandle(', 'void _CreateCacheRequest();', 'int CreateTrueCondition(') | ForEach-Object { $self.IndexOf($_) }
    Check (($at -notcontains -1) -and (($at | Sort-Object) -join ',') -eq ($at -join ',')) 'the COM methods keep their places'
    Check (([regex]::Matches($LiveSource, '(?m)^\s+(void _|\[PreserveSig\])')).Count -eq 2 + 8 + 19) 'the COM interfaces declare every method up to the last one called'
    foreach ($w in ('Start' + '-Process'), ('Invoke' + '-Item'), ('Invoke' + '-Expression'), ('Process]::' + 'Start')) {
        Check (-not $self.Contains($w)) "the script never uses $w"
    }
    Check ($self -match 'TogglePattern\]::Pattern, \[ref\]\$pat') 'the -Page lookup falls back to TogglePattern (sidebar buttons have no Select or Invoke)'
    if ($fail.Count) { $fail | ForEach-Object { Write-Host "FAIL: $_" }; exit 1 }
    Write-Host 'uia_tree self-test: ok'
    exit 0
}

# ---- can this PC read LiveSetting? -------------------------------------------------------------

# One read of one property on the desktop's root element, through the same COM route the dump
# uses. Reads only; touches no window, no Foxglove, no focus.
if ($LiveProbe) {
    try {
        Add-Type -TypeDefinition $LiveSource
        $v = [FoxgloveLab.UiaLive]::Root()
    } catch {
        Write-Host "LiveSetting not read through COM UI Automation: $($_.Exception.Message)"
        exit 3
    }
    if ($null -eq $v) { Write-Host 'LiveSetting not read through COM UI Automation: the desktop root gave no number'; exit 3 }
    Write-Host "LiveSetting read through COM UI Automation: the desktop root says $v (0 off, 1 polite, 2 assertive)"
    exit 0
}

# ---- attach and dump -------------------------------------------------------------------------

if ($env:FOXGLOVE_APP_OK -ne '1') {
    Write-Host 'BLOCKED: this reads a running Foxglove (and turns its AccessKit tree on); On-call runs it with FOXGLOVE_APP_OK=1'
    exit 3
}
if ($Page -and -not (Test-PageName $Page)) { Write-Host "bad -Page '$Page' (one of: $($PageNames -join ', '))"; exit 3 }
if ($ProcessId -le 0) { Write-Host "give -ProcessId of a running $ProcessName"; exit 3 }

$proc = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
if (-not $proc -or $proc.ProcessName -cne $ProcessName) {
    Write-Host "refused: process $ProcessId is not a running process named exactly '$ProcessName'"
    exit 3
}
# Same Windows session and, when Windows tells us the owner, the same user.
if ($proc.SessionId -ne (Get-Process -Id $PID).SessionId) {
    Write-Host "refused: process $ProcessId runs in another Windows session"
    exit 3
}
try {
    $owner = Invoke-CimMethod -InputObject (Get-CimInstance Win32_Process -Filter "ProcessId = $ProcessId") -MethodName GetOwner
    if ($owner.ReturnValue -eq 0 -and ($owner.User -cne $env:USERNAME -or $owner.Domain -cne $env:USERDOMAIN)) {
        Write-Host "refused: process $ProcessId belongs to $($owner.Domain)\$($owner.User), not to you"
        exit 3
    }
} catch { }

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes

# The walk runs in its own runspace so a hung window cannot block this script: the wait below is a
# hard limit, and leaving the process ends the stuck thread.
$walk = {
    param($pid_, $title, $handle, $maxNodes, $deadlineTicks, $recordFn, $page, $liveSource)
    $ErrorActionPreference = 'Stop'
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
    Set-Item -Path function:New-NodeRecord -Value ([scriptblock]::Create($recordFn))
    $script:count = 0
    $script:timedOut = $false
    $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker

    function Get-State($el) {
        try {
            $p = $null
            if ($el.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$p)) {
                return $(if ($p.Current.IsSelected) { 'on' } else { 'off' })
            }
            if ($el.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$p)) {
                return $(if ($p.Current.ToggleState -eq 'On') { 'on' } else { 'off' })
            }
        } catch { }
        return $null
    }

    # UIA_LiveSettingPropertyId (30135) comes from the COM client (the managed one does not know
    # the property): $script:liveMap is runtime id -> value, filled once before the walk. Without
    # it, or for a node it does not hold, liveSetting is null, never a guess.
    $script:liveMap = $null
    $script:liveError = $null
    function Get-Live($el) {
        if ($null -eq $script:liveMap) { return $null }
        try {
            $key = [FoxgloveLab.UiaLive]::Key([int[]]$el.GetRuntimeId())
            $v = 0
            if ($key -and $script:liveMap.TryGetValue($key, [ref]$v)) { return $v }
        } catch { }
        return $null
    }

    function Read-Node($el, [int]$depth) {
        $script:count++
        if ([DateTime]::UtcNow.Ticks -gt $deadlineTicks -or $script:count -gt $maxNodes) { $script:timedOut = $true }
        $c = $el.Current
        $b = $c.BoundingRectangle
        $rect = [ordered]@{ x = 0; y = 0; w = 0; h = 0 }
        if (-not $b.IsEmpty) {
            $rect = [ordered]@{ x = [int]$b.X; y = [int]$b.Y; w = [int]$b.Width; h = [int]$b.Height }
        }
        $kids = @()
        if (-not $script:timedOut -and $depth -lt 40) {
            $k = $walker.GetFirstChild($el)
            while ($null -ne $k -and -not $script:timedOut) {
                $kids += , (Read-Node $k ($depth + 1))
                $k = $walker.GetNextSibling($k)
            }
        }
        $role = $c.ControlType.ProgrammaticName -replace '^ControlType\.', ''
        New-NodeRecord $role $c.Name $c.AutomationId $c.IsEnabled (Get-State $el) $rect $kids (Get-Live $el)
    }

    $cond = New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::ProcessIdProperty), $pid_
    $wins = @([System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $cond))
    $hits = @($wins | Where-Object {
        if ($handle) { $_.Current.NativeWindowHandle -eq [int]$handle } else { $_.Current.Name -ceq $title }
    })
    if ($hits.Count -ne 1) {
        $list = @($wins | ForEach-Object {
            $r = $_.Current.BoundingRectangle
            "handle $($_.Current.NativeWindowHandle) (0x$('{0:X}' -f $_.Current.NativeWindowHandle)) '$($_.Current.Name)' class '$($_.Current.ClassName)' $([int]$r.Width)x$([int]$r.Height)"
        }) -join '; '
        return @{ error = "$($hits.Count) windows match (want exactly 1; pick one with -Handle). Windows of the process: $list" }
    }
    if ($page) {
        # The sidebar entry of that name: the first element of that name that can be selected or
        # invoked (the sidebar comes before the page in the tree).
        $nameCond = New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::NameProperty), $page
        $entry = $null
        foreach ($e in @($hits[0].FindAll([System.Windows.Automation.TreeScope]::Descendants, $nameCond))) {
            $pat = $null
            if ($e.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pat)) {
                $pat.Select(); $entry = $e; break
            }
            if ($e.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pat)) {
                $pat.Invoke(); $entry = $e; break
            }
            # Foxglove's sidebar buttons are AccessKit toggled buttons: UIA gives them TogglePattern
            # only (no Select, no Invoke). Toggle only an entry that is off, so an already open page
            # stays open.
            if ($e.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pat)) {
                if ($pat.Current.ToggleState -ne 'On') { $pat.Toggle() }
                $entry = $e; break
            }
        }
        if (-not $entry) {
            $seen = @(foreach ($e in @($hits[0].FindAll([System.Windows.Automation.TreeScope]::Descendants, $nameCond))) {
                "$($e.Current.ControlType.ProgrammaticName -replace '^ControlType\.', '') [" + (($e.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName -replace 'Identifiers\.Pattern$', '' }) -join ',') + ']'
            }) -join '; '
            return @{ error = "no sidebar entry named '$page' that can be selected, invoked or toggled (elements of that name: $seen)" }
        }
        # One short settle for the page to redraw and the tree to update (a single wait, no loop).
        Start-Sleep -Milliseconds 700
    }
    try {
        Add-Type -TypeDefinition $liveSource
        $script:liveMap = [FoxgloveLab.UiaLive]::Map([IntPtr][int]$hits[0].Current.NativeWindowHandle, $maxNodes * 4, $deadlineTicks)
    } catch {
        $script:liveMap = $null
        $script:liveError = "$($_.Exception.Message)"
    }
    $tree = Read-Node $hits[0] 0
    $liveRead = 0
    if ($null -ne $script:liveMap) { $liveRead = $script:liveMap.Count }
    @{ tree = $tree; timedOut = $script:timedOut; count = $script:count; liveRead = $liveRead; liveError = $script:liveError }
}

$handleNum = ''
if ($Handle) {
    try { $handleNum = [string][Convert]::ToInt32($Handle, $(if ($Handle -match '^0x') { 16 } else { 10 })) }
    catch { Write-Host "bad -Handle '$Handle'"; exit 3 }
}
$deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSec)
$ps = [powershell]::Create()
[void]$ps.AddScript($walk.ToString()).AddArgument($ProcessId).AddArgument($Title).AddArgument($handleNum).AddArgument($MaxNodes).AddArgument($deadline.Ticks).AddArgument(${function:New-NodeRecord}.ToString()).AddArgument($Page).AddArgument($LiveSource)
$async = $ps.BeginInvoke()
if (-not $async.AsyncWaitHandle.WaitOne(($TimeoutSec + 5) * 1000)) {
    Write-Host "timed out: the window did not answer within $TimeoutSec s (hung?)"
    exit 4
}
$out = $ps.EndInvoke($async)
if ($ps.Streams.Error.Count) { Write-Host "UIA error: $($ps.Streams.Error[0])"; exit 3 }
$res = $out | Where-Object { $_ -is [hashtable] } | Select-Object -First 1
if (-not $res) { Write-Host 'no result from the UIA walk'; exit 3 }
if ($res.error) { Write-Host $res.error; exit 3 }

$tree = $res.tree
$json = $tree | ConvertTo-Json -Depth 60
if ($OutFile) { [System.IO.File]::WriteAllText($OutFile, $json, (New-Object System.Text.UTF8Encoding($false))) } else { $json }
if ($res.liveError) { Write-Host "LiveSetting not read (COM UI Automation): $($res.liveError); every node says null" }
elseif ($res.liveRead -eq 0) { Write-Host 'LiveSetting not read (COM UI Automation listed no element); every node says null' }
if ($res.timedOut) { Write-Host "timed out after $($res.count) nodes (-TimeoutSec / -MaxNodes)"; exit 4 }

$missing = @(Find-Missing $tree $Expect)
if ($missing.Count) {
    $missing | ForEach-Object { Write-Host "MISSING: $_" }
    exit 2
}
exit 0
