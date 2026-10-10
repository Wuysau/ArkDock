param([Parameter(Mandatory)][int]$Port, [Parameter(Mandatory)][int]$OwnerId)
$ErrorActionPreference = 'Stop'
$listeners = @(Get-NetTCPConnection -State Listen -LocalPort $Port)
if ($listeners.Count -eq 0) { throw 'Owned debugger listener missing' }
foreach ($listener in $listeners) {
    $probeProcessId = [int]$listener.OwningProcess
    $verified = $false
    for ($depth = 0; $depth -lt 12 -and $probeProcessId -gt 0; $depth++) {
        if ($probeProcessId -eq $OwnerId) { $verified = $true; break }
        $metadata = Get-CimInstance Win32_Process -Filter "ProcessId=$probeProcessId"
        if (-not $metadata) { break }
        $probeProcessId = [int]$metadata.ParentProcessId
    }
    if (-not $verified) { throw 'Debugger does not belong to the explicitly owned ArkDock process' }
}
Write-Output 'PASS owned debugger ancestry'
