# Dumps the UI Automation tree of an ALREADY RUNNING Foxglove window (#1376), as JSON, and checks
# that expected nodes are in it. ATTACH ONLY: this script never starts Foxglove (or any program);
# it reads the process you name by id. Windows PowerShell 5.1, UIAutomationClient.
#
#   uia_tree.ps1 -ProcessId 1234 [-Title Foxglove] [-Expect 'Button:Start','RadioButton:Temperature:on']
#                [-TimeoutSec 20] [-MaxNodes 4000] [-OutFile tree.json]
#   uia_tree.ps1 -SelfTest        (pure parts only: JSON shape, expectation matching; no UIA, no process)
#
# Needs FOXGLOVE_APP_OK=1 (On-call's OK, like every lab case that touches Foxglove): the first read
# of the tree turns AccessKit on in that process until it restarts, which changes its CPU profile.
# Point it at a lab instance (isolated FOXGLOVE_CONFIG_DIR) unless On-call says otherwise.
#
# The process must be named exactly `foxglove` (never a wildcard). Window: the top-level window of
# that process whose name equals -Title (default "Foxglove", the Settings window; the overlay is
# named "Foxglove overlay").
#
# -Expect items are Role:Name or Role:Name:state. Role is a UIA control type (Button, RadioButton,
# Group, Text, ...), Name the exact accessible name, state `on` (selected or toggled on) or `off`.
#
# Exit codes: 0 all expectations met (or none given), 2 an expectation is missing, 3 refused or
# window not found, 4 timed out. Nothing is clicked, typed or focused; the tree is only read.
param(
    [int]$ProcessId = 0,
    [string]$Title = 'Foxglove',
    [string[]]$Expect = @(),
    [int]$TimeoutSec = 20,
    [int]$MaxNodes = 4000,
    [string]$OutFile = '',
    [switch]$SelfTest
)
$ErrorActionPreference = 'Stop'

# ---- pure parts (tested by -SelfTest) --------------------------------------------------------

# One dumped node as a plain object: the shape of the JSON.
function New-NodeRecord($role, $name, $autoId, $enabled, $state, $rect, $children) {
    [ordered]@{
        role = $role; name = $name; automationId = $autoId; enabled = $enabled
        state = $state; rect = $rect; children = @($children)
    }
}

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
    $json = $tree | ConvertTo-Json -Depth 20
    $back = $json | ConvertFrom-Json
    Check ($back.children[1].name -eq 'Start' -and $back.children[0].children.Count -eq 2) 'JSON keeps the shape'
    # Attach only: the script text starts nothing.
    $self = Get-Content -LiteralPath $PSCommandPath -Raw
    foreach ($w in ('Start' + '-Process'), ('Invoke' + '-Item'), ('Invoke' + '-Expression'), ('Process]::' + 'Start')) {
        Check (-not $self.Contains($w)) "the script never uses $w"
    }
    if ($fail.Count) { $fail | ForEach-Object { Write-Host "FAIL: $_" }; exit 1 }
    Write-Host 'uia_tree self-test: ok'
    exit 0
}

# ---- attach and dump -------------------------------------------------------------------------

if ($env:FOXGLOVE_APP_OK -ne '1') {
    Write-Host 'BLOCKED: this reads a running Foxglove (and turns its AccessKit tree on); On-call runs it with FOXGLOVE_APP_OK=1'
    exit 3
}
if ($ProcessId -le 0) { Write-Host 'give -ProcessId of a running foxglove'; exit 3 }

$proc = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
if (-not $proc -or $proc.ProcessName -cne 'foxglove') {
    Write-Host "refused: process $ProcessId is not a running process named exactly 'foxglove'"
    exit 3
}

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSec)
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

function Read-Node($el, [int]$depth) {
    $script:count++
    if ([DateTime]::UtcNow -gt $deadline -or $script:count -gt $MaxNodes) { $script:timedOut = $true }
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
    New-NodeRecord $role $c.Name $c.AutomationId $c.IsEnabled (Get-State $el) $rect $kids
}

$cond = New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::ProcessIdProperty), $ProcessId
$wins = [System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)
$win = $null
foreach ($w in $wins) { if ($w.Current.Name -ceq $Title) { $win = $w; break } }
if (-not $win) {
    $names = @($wins | ForEach-Object { "'" + $_.Current.Name + "'" }) -join ', '
    Write-Host "no window named '$Title' in process $ProcessId (windows: $names)"
    exit 3
}

$tree = Read-Node $win 0
$json = $tree | ConvertTo-Json -Depth 60
if ($OutFile) { [System.IO.File]::WriteAllText($OutFile, $json, (New-Object System.Text.UTF8Encoding($false))) } else { $json }
if ($script:timedOut) { Write-Host "timed out after $script:count nodes (-TimeoutSec / -MaxNodes)"; exit 4 }

$missing = @(Find-Missing $tree $Expect)
if ($missing.Count) {
    $missing | ForEach-Object { Write-Host "MISSING: $_" }
    exit 2
}
exit 0
