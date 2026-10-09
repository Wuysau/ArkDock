# Actual independent Windows Forms targets for A assertions. No ArkUI or preview substitute.
param([string]$ProjectPath)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::SetHighDpiMode([System.Windows.Forms.HighDpiMode]::PerMonitorV2) | Out-Null
Add-Type -ReferencedAssemblies @('System.Windows.Forms','System.Windows.Forms.Primitives','System.ComponentModel.Primitives','System.Runtime','System.Console','System.Threading.Thread') @'
using System;
using System.Threading;
using System.Windows.Forms;
public class ArkDockFixtureForm : Form {
  protected override void WndProc(ref Message message) {
    if (message.Msg == 0x8001) {
      Console.WriteLine("{\"pauseReady\":true}");
      Thread.Sleep(Math.Clamp(message.WParam.ToInt32(), 1, 2500));
      return;
    }
    base.WndProc(ref message);
  }
}
'@
$mainTarget = [ArkDockFixtureForm]::new()
$mainTarget.Text = 'ArkDock Windows 验收目标'
$mainTarget.StartPosition = 'Manual'
$mainTarget.Location = [System.Drawing.Point]::new(30, 80)
$mainTarget.Size = [System.Drawing.Size]::new(600, 600)
$label = [System.Windows.Forms.Label]::new()
$label.Text = '普通桌面窗口联动验收；此窗口不是鸿蒙预览。'
$label.AutoSize = $true
$label.Location = [System.Drawing.Point]::new(24, 32)
$mainTarget.Controls.Add($label)
$tabs = [System.Windows.Forms.TabControl]::new()
$tabs.Location = [System.Drawing.Point]::new(20, 100)
$tabs.Size = [System.Drawing.Size]::new(540, 300)
$selectedPath = if ($ProjectPath) { Join-Path $ProjectPath 'entry/src/main/ets/pages/Index.ets' } else { Join-Path $PSScriptRoot '../fixtures/harmony-app/entry/src/main/ets/pages/Index.ets' }
$currentTab = [System.Windows.Forms.TabPage]::new([System.IO.Path]::GetFullPath($selectedPath))
$otherTab = [System.Windows.Forms.TabPage]::new('未选中的其他标签')
$tabs.TabPages.Add($currentTab)
$tabs.TabPages.Add($otherTab)
$mainTarget.Controls.Add($tabs)
$secondTarget = [System.Windows.Forms.Form]::new()
$secondTarget.Text = 'ArkDock 同进程第二个验收窗口'
$secondTarget.StartPosition = 'Manual'
$secondTarget.Location = [System.Drawing.Point]::new(50, 120)
$secondTarget.Size = [System.Drawing.Size]::new(400, 420)
$secondTarget.Show()
$mainTarget.Show()
@{ pid = $PID; first = $mainTarget.Handle.ToInt64(); second = $secondTarget.Handle.ToInt64() } | ConvertTo-Json -Compress
[System.Windows.Forms.Application]::Run($mainTarget)
$secondTarget.Dispose()
$mainTarget.Dispose()
