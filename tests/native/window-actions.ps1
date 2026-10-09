param(
  [Parameter(Mandatory)][ValidateSet('find','info','move','minimize','restore','maximize','close','click','drag','foreground','pause')][string]$Action,
  [long]$Handle, [int]$ProcessId, [int]$OutlinePid, [int]$X, [int]$Y, [int]$Width, [int]$Height, [int]$EndX, [int]$EndY
)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Collections.Generic;
using System.Text;
using System.Runtime.InteropServices;
public static class ArkDockWindowTest {
  public delegate bool EnumCallback(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCallback cb, IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder text, int size);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder text, int size);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint command);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int index);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, uint attribute, out Rect rect, uint size);
  public static long[] Higher(IntPtr h) { var list=new List<long>(); for(var i=0;i<1024;i++) {h=GetWindow(h,3); if(h==IntPtr.Zero) break; if(IsWindowVisible(h)) list.Add(h.ToInt64());} return list.ToArray(); }
  public static long[] Outlines(uint pid) {
    var list=new List<long>();
    EnumWindows((h,p) => { uint observed; GetWindowThreadProcessId(h,out observed); var name=new StringBuilder(256); GetClassName(h,name,256);
      if(observed==pid && IsWindowVisible(h) && name.ToString().StartsWith("ArkDockCandidateOutline-")) list.Add(h.ToInt64()); return true;
    },IntPtr.Zero); return list.ToArray();
  }
  public static long Find(uint pid) {
    long result=0;
    EnumWindows((h,p) => { uint observed; GetWindowThreadProcessId(h,out observed);
      var title=new StringBuilder(1024); GetWindowText(h,title,title.Capacity);
      if(observed==pid && IsWindowVisible(h) && GetWindow(h,4)==IntPtr.Zero && title.ToString()=="ArkDock") { result=h.ToInt64(); return false; }
      return true;
    },IntPtr.Zero); return result;
  }
  [StructLayout(LayoutKind.Sequential)] public struct Rect { public int left, top, right, bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct Point { public int x, y; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref Point p);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr z, int x, int y, int w, int height, uint flags);
  [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr h, int command);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr value);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
}
'@
$previousDpi = [ArkDockWindowTest]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))
try {
  $windowHandle = [IntPtr]::new($Handle)
  $outlineDuringDrag = @()
  switch ($Action) {
    'find' { @{ handle=[ArkDockWindowTest]::Find($ProcessId) } | ConvertTo-Json -Compress; return }
    'move' { if (-not [ArkDockWindowTest]::SetWindowPos($windowHandle,[IntPtr]::Zero,$X,$Y,$Width,$Height,0x14)) { throw 'SetWindowPos failed' } }
    'minimize' { [ArkDockWindowTest]::ShowWindowAsync($windowHandle,6) | Out-Null }
    'restore' { [ArkDockWindowTest]::ShowWindowAsync($windowHandle,4) | Out-Null }
    'maximize' { [ArkDockWindowTest]::ShowWindowAsync($windowHandle,3) | Out-Null }
    'close' { [ArkDockWindowTest]::PostMessage($windowHandle,0x10,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null }
    'foreground' { [ArkDockWindowTest]::SetForegroundWindow($windowHandle) | Out-Null }
    'pause' { [ArkDockWindowTest]::PostMessage($windowHandle,0x8001,[IntPtr]::new(2000),[IntPtr]::Zero) | Out-Null; @{queued=$true} | ConvertTo-Json -Compress; return }
    { $_ -in 'click','drag' } {
      [ArkDockWindowTest]::SetCursorPos($X,$Y) | Out-Null
      Start-Sleep -Milliseconds 100
      [ArkDockWindowTest]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
      try {
        Start-Sleep -Milliseconds 250
        if ($Action -eq 'drag') {
          for ($step = 1; $step -le 24; $step++) {
            [ArkDockWindowTest]::SetCursorPos(($X + ($EndX-$X)*$step/24),($Y + ($EndY-$Y)*$step/24)) | Out-Null
            Start-Sleep -Milliseconds 25
          }
          Start-Sleep -Milliseconds 150
          $outlineDuringDrag = @([ArkDockWindowTest]::Outlines($OutlinePid) | ForEach-Object { @{ handle=$_;extended=[ArkDockWindowTest]::GetWindowLong([IntPtr]::new($_),-20) } })
        }
      } finally { [ArkDockWindowTest]::mouse_event(4,0,0,0,[UIntPtr]::Zero) }
    }
  }
  $rect = [ArkDockWindowTest+Rect]::new()
  $frame = [ArkDockWindowTest+Rect]::new()
  $origin = [ArkDockWindowTest+Point]::new()
  [ArkDockWindowTest]::GetWindowRect($windowHandle,[ref]$rect) | Out-Null
  [ArkDockWindowTest]::ClientToScreen($windowHandle,[ref]$origin) | Out-Null
  [ArkDockWindowTest]::DwmGetWindowAttribute($windowHandle,9,[ref]$frame,16) | Out-Null
  $higher = @([ArkDockWindowTest]::Higher($windowHandle) | ForEach-Object {
    $higherRect = [ArkDockWindowTest+Rect]::new()
    [ArkDockWindowTest]::DwmGetWindowAttribute([IntPtr]::new($_),9,[ref]$higherRect,16) | Out-Null
    @{handle=$_;left=$higherRect.left;top=$higherRect.top;right=$higherRect.right;bottom=$higherRect.bottom}
  })
  @{ left=$rect.left; top=$rect.top; right=$rect.right; bottom=$rect.bottom;
     outlineDuringDrag=$outlineDuringDrag;outlineVisible=@([ArkDockWindowTest]::Outlines($OutlinePid)).Count;
     frameLeft=$frame.left;frameTop=$frame.top;frameRight=$frame.right;frameBottom=$frame.bottom;
     owner=[ArkDockWindowTest]::GetWindow($windowHandle,4).ToInt64();style=[ArkDockWindowTest]::GetWindowLong($windowHandle,-16);extended=[ArkDockWindowTest]::GetWindowLong($windowHandle,-20);higher=$higher;
     clientX=$origin.x; clientY=$origin.y; dpi=[ArkDockWindowTest]::GetDpiForWindow($windowHandle);
     visible=[ArkDockWindowTest]::IsWindowVisible($windowHandle); minimized=[ArkDockWindowTest]::IsIconic($windowHandle);
     maximized=[ArkDockWindowTest]::IsZoomed($windowHandle); foreground=[ArkDockWindowTest]::GetForegroundWindow().ToInt64() } | ConvertTo-Json -Compress
} finally { [ArkDockWindowTest]::SetThreadDpiAwarenessContext($previousDpi) | Out-Null }
