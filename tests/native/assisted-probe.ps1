# Read-only accessibility metadata for the user's exact selected Codex window.
# No discovery across unrelated windows, input, activation, values, or chat text.
param(
  [Parameter(Mandatory=$true)][long]$WindowHandle,
  [Parameter(Mandatory=$true)][int]$TargetPid,
  [Parameter(Mandatory=$true)][string]$StartedAt
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ArkDockAssistedProbe {
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr handle, out uint pid);
}
'@
$probeProcess = Get-Process -Id $TargetPid
if ($probeProcess.StartTime.ToUniversalTime().ToFileTimeUtc().ToString() -ne $StartedAt) { throw 'Selected process lifecycle changed' }
[uint32]$probeWindowPid = 0
if ([ArkDockAssistedProbe]::GetWindowThreadProcessId([IntPtr]$WindowHandle,[ref]$probeWindowPid) -eq 0 -or $probeWindowPid -ne $TargetPid) { throw 'Selected HWND no longer belongs to the selected process' }
if ($probeProcess.Path -notmatch '\\WindowsApps\\OpenAI\.Codex_[^\\]+\\app\\ChatGPT\.exe$') { throw 'Selected executable is not the expected installed Codex Desktop package' }
$probeRoot = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$WindowHandle)
if ($probeRoot.Current.ProcessId -ne $TargetPid) { throw 'UIA root process differs from the selected process' }
$probeWalker = [System.Windows.Automation.TreeWalker]::RawViewWalker
$probeQueue = [System.Collections.Generic.Queue[object]]::new()
$probeQueue.Enqueue(@($probeRoot,0))
$probeCount = 0
$probeDeadline = [DateTime]::UtcNow.AddSeconds(3)
$probeCandidates = @()
$probeTypes = @{}
while ($probeQueue.Count -gt 0 -and $probeCount -lt 256 -and [DateTime]::UtcNow -lt $probeDeadline) {
  $probePair = $probeQueue.Dequeue()
  $probeElement = $probePair[0]
  $probeDepth = $probePair[1]
  $probeChild = $probeWalker.GetFirstChild($probeElement)
  while ($null -ne $probeChild -and $probeCount -lt 256 -and [DateTime]::UtcNow -lt $probeDeadline) {
    $probeCount++
    $probeCurrent = $probeChild.Current
    $probeType = $probeCurrent.ControlType.ProgrammaticName
    $probeTypes[$probeType]++
    if ($probeType -eq 'ControlType.Edit' -and -not $probeCurrent.IsPassword) {
      $probeValue = $null
      $probeHasValue = $probeChild.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern,[ref]$probeValue)
      $probeCandidates += [PSCustomObject]@{
        AutomationId = $probeCurrent.AutomationId
        ClassName = $probeCurrent.ClassName
        Framework = $probeCurrent.FrameworkId
        ProcessId = $probeCurrent.ProcessId
        Enabled = $probeCurrent.IsEnabled
        Offscreen = $probeCurrent.IsOffscreen
        ValuePattern = $probeHasValue
        ReadOnly = if ($probeHasValue) { $probeValue.Current.IsReadOnly } else { $null }
        Depth = $probeDepth
      }
    }
    if ($probeDepth -lt 16 -and $probeType -in @('ControlType.Pane','ControlType.Group','ControlType.Custom','ControlType.Document','ControlType.ToolBar')) {
      $probeQueue.Enqueue(@($probeChild,($probeDepth+1)))
    }
    $probeChild = $probeWalker.GetNextSibling($probeChild)
  }
}
$probeProcess.Refresh()
if ($probeProcess.HasExited -or $probeProcess.StartTime.ToUniversalTime().ToFileTimeUtc().ToString() -ne $StartedAt) { throw 'Selected process changed during inspection' }
[PSCustomObject]@{
  status = 'BLOCKED'
  observedAt = [DateTime]::UtcNow.ToString('o')
  application = 'Codex Desktop'
  packageVersion = ([regex]::Match($probeProcess.Path,'OpenAI\.Codex_([^_]+)_')).Groups[1].Value
  nodeCount = $probeCount
  pendingContainers = $probeQueue.Count
  controlTypes = $probeTypes
  editableMetadata = $probeCandidates
  writes = 0
  reason = 'Metadata inspection alone does not authorize filling; no verified composer contract or current-thread identity is established.'
} | ConvertTo-Json -Depth 6
