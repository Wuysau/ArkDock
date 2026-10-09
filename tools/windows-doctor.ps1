# Read-only inventory of installed Windows prerequisites; no downloads, installs or project code.
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$system = Get-CimInstance Win32_OperatingSystem
$vsProbe = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$compilers = @()
if (Test-Path -LiteralPath $vsProbe) {
  $installations = & $vsProbe -products '*' -format json | ConvertFrom-Json
  foreach ($installation in $installations) {
    $versionRoot = Join-Path $installation.installationPath 'VC/Tools/MSVC'
    if (Test-Path -LiteralPath $versionRoot) {
      foreach ($version in @(Get-ChildItem -LiteralPath $versionRoot -Directory | Select-Object -First 16)) {
        if (Test-Path -LiteralPath (Join-Path $version.FullName 'bin/HostX64/x64/cl.exe')) { $compilers += $version.Name }
      }
    }
  }
}
$sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/Lib'
$windowsSdks = @(if (Test-Path -LiteralPath $sdkRoot) { Get-ChildItem -LiteralPath $sdkRoot -Directory | Select-Object -First 16 -ExpandProperty Name })
$webViewKey = 'SOFTWARE/Microsoft/EdgeUpdate/Clients/{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$webView = @(Get-ItemProperty -LiteralPath "HKLM:/$webViewKey",("HKLM:/" + $webViewKey.Replace('SOFTWARE/','SOFTWARE/WOW6432Node/')),"HKCU:/$webViewKey" -ErrorAction SilentlyContinue | Select-Object -ExpandProperty pv)
Add-Type -AssemblyName System.Windows.Forms
@{ caption=$system.Caption; version=$system.Version; build=[int]$system.BuildNumber; architecture=$system.OSArchitecture;
   msvc=$compilers; windowsSdk=$windowsSdks; webView2=$webView; monitors=[System.Windows.Forms.Screen]::AllScreens.Count;
   interactive=[Environment]::UserInteractive } | ConvertTo-Json -Compress
