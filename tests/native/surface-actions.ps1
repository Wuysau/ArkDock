param(
  [Parameter(Mandatory)][ValidateSet('list','probe','menu','taskbar-created','capture')][string]$Action,
  [Parameter(Mandatory)][int]$ProcessId,
  [long]$Handle, [int]$MenuId, [string]$OutputPath
)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class ArkDockSurfaceTest {
  public delegate bool EnumCallback(IntPtr h,IntPtr p);
  [StructLayout(LayoutKind.Sequential)] public struct Rect { public int left,top,right,bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct Point { public int x,y; }
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCallback cb,IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
  [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h,StringBuilder text,int size);
  [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h,StringBuilder text,int size);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out Rect r);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h,int index);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h,uint msg,IntPtr w,IntPtr l);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint msg,IntPtr w,IntPtr l);
  [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern uint RegisterWindowMessage(string value);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(Point point);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr value);
  public static long[] List(uint expected) {
    var list=new List<long>(); EnumWindows((h,p)=>{uint pid;GetWindowThreadProcessId(h,out pid);
      var name=new StringBuilder(256);GetClassName(h,name,256);
      if(pid==expected && (name.ToString().StartsWith("ArkDockOrb-") || name.ToString().StartsWith("ArkDockMagneticParticles-"))) list.Add(h.ToInt64());
      return true;
    },IntPtr.Zero);return list.ToArray();
  }
}
'@
$taskDpi=[ArkDockSurfaceTest]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))
try {
  if ($Action -eq 'list') {
    $records=@([ArkDockSurfaceTest]::List($ProcessId) | ForEach-Object {
      $windowHandle=[IntPtr]::new($_)
      $text=[System.Text.StringBuilder]::new(256)
      $class=[System.Text.StringBuilder]::new(256)
      [ArkDockSurfaceTest]::GetWindowText($windowHandle,$text,256) | Out-Null
      [ArkDockSurfaceTest]::GetClassName($windowHandle,$class,256) | Out-Null
      $rect=[ArkDockSurfaceTest+Rect]::new()
      [ArkDockSurfaceTest]::GetWindowRect($windowHandle,[ref]$rect) | Out-Null
      @{handle=$_;title=$text.ToString();class=$class.ToString();visible=[ArkDockSurfaceTest]::IsWindowVisible($windowHandle);left=$rect.left;top=$rect.top;right=$rect.right;bottom=$rect.bottom;dpi=[ArkDockSurfaceTest]::GetDpiForWindow($windowHandle);extended=[ArkDockSurfaceTest]::GetWindowLong($windowHandle,-20)}
    })
    ConvertTo-Json -InputObject $records -Compress
    return
  }
  $windowHandle=[IntPtr]::new($Handle)
  [uint32]$observedProcessId=0
  [ArkDockSurfaceTest]::GetWindowThreadProcessId($windowHandle,[ref]$observedProcessId) | Out-Null
  if ($observedProcessId -ne $ProcessId) { throw 'Surface action requires the explicitly owned process' }
  $surfaceClass=[System.Text.StringBuilder]::new(256)
  [ArkDockSurfaceTest]::GetClassName($windowHandle,$surfaceClass,256) | Out-Null
  if (-not $surfaceClass.ToString().StartsWith('ArkDockOrb-')) { throw 'Surface action requires ArkDock native surface class' }
  switch ($Action) {
    'menu' {
      if ($MenuId -lt 1 -or $MenuId -gt 7) { throw 'Invalid surface menu ID' }
      if (-not [ArkDockSurfaceTest]::PostMessage($windowHandle,0x111,[IntPtr]::new($MenuId),[IntPtr]::Zero)) { throw 'Owned menu message failed' }
      @{posted=$true} | ConvertTo-Json -Compress
    }
    'taskbar-created' {
      $message=[ArkDockSurfaceTest]::RegisterWindowMessage('TaskbarCreated')
      [ArkDockSurfaceTest]::PostMessage($windowHandle,$message,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null
      @{posted=$true} | ConvertTo-Json -Compress
    }
    'probe' {
      $rect=[ArkDockSurfaceTest+Rect]::new()
      [ArkDockSurfaceTest]::GetWindowRect($windowHandle,[ref]$rect) | Out-Null
      $center=[ArkDockSurfaceTest+Point]::new()
      $center.x=($rect.left+$rect.right)/2;$center.y=($rect.top+$rect.bottom)/2
      $corner=[ArkDockSurfaceTest+Point]::new()
      $corner.x=$rect.left+1;$corner.y=$rect.top+1
      $pointBits=([long]($center.x -band 0xffff)) -bor (([long]($center.y -band 0xffff)) -shl 16)
      $centerHandle=[ArkDockSurfaceTest]::WindowFromPoint($center)
      $centerClass=[Text.StringBuilder]::new(256)
      [uint32]$centerProcessId=0
      [ArkDockSurfaceTest]::GetClassName($centerHandle,$centerClass,256) | Out-Null
      [ArkDockSurfaceTest]::GetWindowThreadProcessId($centerHandle,[ref]$centerProcessId) | Out-Null
      @{centerWindow=$centerHandle.ToInt64();centerClass=$centerClass.ToString();centerProcessId=$centerProcessId;centerX=$center.x;centerY=$center.y;cornerWindow=[ArkDockSurfaceTest]::WindowFromPoint($corner).ToInt64();mouseActivate=[ArkDockSurfaceTest]::SendMessage($windowHandle,0x21,[IntPtr]::Zero,[IntPtr]::Zero).ToInt64();hitTest=[ArkDockSurfaceTest]::SendMessage($windowHandle,0x84,[IntPtr]::Zero,[IntPtr]::new($pointBits)).ToInt64()} | ConvertTo-Json -Compress
    }
    'capture' {
      if (-not $OutputPath) { throw 'OutputPath required' }
      Add-Type -AssemblyName System.Drawing
      $rect=[ArkDockSurfaceTest+Rect]::new()
      [ArkDockSurfaceTest]::GetWindowRect($windowHandle,[ref]$rect) | Out-Null
      $image=[System.Drawing.Bitmap]::new($rect.right-$rect.left,$rect.bottom-$rect.top)
      $graphics=[System.Drawing.Graphics]::FromImage($image)
      try {
        $graphics.CopyFromScreen($rect.left,$rect.top,0,0,$image.Size)
        $image.Save([System.IO.Path]::GetFullPath($OutputPath),[System.Drawing.Imaging.ImageFormat]::Png)
      } finally { $graphics.Dispose();$image.Dispose() }
      @{path=[System.IO.Path]::GetFullPath($OutputPath)} | ConvertTo-Json -Compress
    }
  }
} finally { [ArkDockSurfaceTest]::SetThreadDpiAwarenessContext($taskDpi) | Out-Null }
