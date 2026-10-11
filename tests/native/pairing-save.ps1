# Save only the native file dialog belonging to the explicitly owned ArkDock.
param(
  [Parameter(Mandatory=$true)][int]$OwnerPid,
  [Parameter(Mandatory=$true)][string]$OwnerExe,
  [Parameter(Mandatory=$true)][string]$Destination
)
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$taskDirectory = [IO.Path]::GetFullPath((Join-Path $taskRoot '.local/agent-delivery')) + [IO.Path]::DirectorySeparatorChar
$taskPairDirectory = [IO.Path]::GetFullPath((Join-Path $taskRoot 'artifacts/agent-delivery')) + [IO.Path]::DirectorySeparatorChar
$taskDestination = [IO.Path]::GetFullPath($Destination)
if ((-not $taskDestination.StartsWith($taskDirectory,[StringComparison]::OrdinalIgnoreCase) -and -not $taskDestination.StartsWith($taskPairDirectory,[StringComparison]::OrdinalIgnoreCase)) -or [IO.File]::Exists($taskDestination)) { throw 'Only a new pairing file inside an explicitly owned task directory may be saved' }
$taskProcess = Get-Process -Id $OwnerPid
if (-not [string]::Equals($taskProcess.Path,[IO.Path]::GetFullPath($OwnerExe),[StringComparison]::OrdinalIgnoreCase)) { throw 'Desktop executable identity changed' }
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
public static class ArkDockPairDialog {
  public delegate bool Callback(IntPtr handle, IntPtr data);
  [DllImport("user32.dll")] static extern bool EnumWindows(Callback callback, IntPtr data);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr handle, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr handle, StringBuilder value, int count);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr handle);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr handle,uint message,IntPtr wparam,IntPtr lparam);
  [StructLayout(LayoutKind.Sequential)] struct Rect {public int left,top,right,bottom;}
  [StructLayout(LayoutKind.Sequential)] struct Point {public int x,y;}
  [StructLayout(LayoutKind.Sequential)] struct GuiInfo {public int size;public uint flags;public IntPtr active,focus,capture,menuOwner,moveSize,caret;public Rect caretRect;}
  [StructLayout(LayoutKind.Sequential)] struct Mouse {public int x,y;public uint data,flags,time;public IntPtr extra;}
  [StructLayout(LayoutKind.Sequential)] struct Key {public ushort vk,scan;public uint flags,time;public IntPtr extra;}
  [StructLayout(LayoutKind.Explicit)] struct InputData {[FieldOffset(0)]public Mouse mouse;[FieldOffset(0)]public Key key;}
  [StructLayout(LayoutKind.Sequential)] struct Input {public uint type;public InputData data;}
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr handle,out Rect rect);
  [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr handle,uint flag);
  [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point point);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern bool GetGUIThreadInfo(uint thread,ref GuiInfo info);
  [DllImport("user32.dll")] static extern bool SetCursorPos(int x,int y);
  [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr handle,IntPtr after,int x,int y,int width,int height,uint flags);
  [DllImport("user32.dll")] static extern uint SendInput(uint count,Input[] inputs,int size);
  [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr handle,uint message,IntPtr wparam,StringBuilder text,uint flags,uint timeout,out IntPtr result);
  static void Owned(IntPtr control,IntPtr dialog,uint wanted) {uint pid;GetWindowThreadProcessId(control,out pid);if(pid!=wanted||GetAncestor(control,2)!=dialog)throw new Exception("Native control ownership changed");}
  static void Focused(IntPtr control,IntPtr dialog,uint wanted) {Owned(control,dialog,wanted);var info=new GuiInfo{size=Marshal.SizeOf(typeof(GuiInfo))};if(GetForegroundWindow()!=dialog||!GetGUIThreadInfo(0,ref info)||info.focus!=control)throw new Exception("Owned filename field lost actual keyboard focus");}
  static void Send(params Input[] inputs) {if(SendInput((uint)inputs.Length,inputs,Marshal.SizeOf(typeof(Input)))!=inputs.Length)throw new Exception("Owned dialog input refused");}
  static Input KeyInput(ushort vk,ushort scan,uint flags) {return new Input{type=1,data=new InputData{key=new Key{vk=vk,scan=scan,flags=flags}}};}
  public static void Click(IntPtr control,IntPtr dialog,uint wanted) {
    Owned(control,dialog,wanted);SetWindowPos(dialog,IntPtr.Zero,0,0,0,0,0x53);Rect bounds;if(!GetWindowRect(control,out bounds))throw new Exception("Owned control geometry unavailable");
    var point=new Point{x=(bounds.left+bounds.right)/2,y=(bounds.top+bounds.bottom)/2};if(GetAncestor(WindowFromPoint(point),2)!=dialog)throw new Exception("Owned save dialog is covered; no click");
    SetCursorPos(point.x,point.y);Send(new Input{data=new InputData{mouse=new Mouse{flags=2}}},new Input{data=new InputData{mouse=new Mouse{flags=4}}});
  }
  public static void TypeFilename(IntPtr control,IntPtr dialog,uint wanted,string path) {
    Click(control,dialog,wanted);var deadline=DateTime.UtcNow.AddSeconds(3);bool ready=false;
    while(DateTime.UtcNow<deadline){try{Focused(control,dialog,wanted);ready=true;break;}catch{Thread.Sleep(10);}}
    if(!ready)throw new Exception("Owned filename control focus not acknowledged");
    Send(KeyInput(0x11,0,0),KeyInput(0x41,0,0),KeyInput(0x41,0,2),KeyInput(0x11,0,2));
    foreach(char value in path){Focused(control,dialog,wanted);Send(KeyInput(0,value,4),KeyInput(0,value,6));}
    deadline=DateTime.UtcNow.AddSeconds(3);
    while(DateTime.UtcNow<deadline){Focused(control,dialog,wanted);var actual=new StringBuilder(4096);IntPtr result;if(SendMessageTimeout(control,0x0D,new IntPtr(actual.Capacity),actual,2,500,out result)!=IntPtr.Zero&&actual.ToString()==path)return;Thread.Sleep(10);}
    throw new Exception("Owned filename value did not match requested path; save refused");
  }
  public static long[] Find(uint wanted) {
    var handles=new List<long>();
    EnumWindows((handle,data)=>{uint pid;GetWindowThreadProcessId(handle,out pid);if(pid==wanted&&IsWindowVisible(handle)){var kind=new StringBuilder(128);GetClassName(handle,kind,128);if(kind.ToString()=="#32770")handles.Add(handle.ToInt64());}return true;},IntPtr.Zero);
    return handles.ToArray();
  }
}
'@
$taskDeadline = [DateTime]::UtcNow.AddSeconds(10)
$taskDialogs = @()
while ([DateTime]::UtcNow -lt $taskDeadline) {
  $taskDialogs = @([ArkDockPairDialog]::Find([uint32]$OwnerPid))
  if ($taskDialogs.Count -gt 0) { break }
  Start-Sleep -Milliseconds 50
}
if ($taskDialogs.Count -ne 1) { throw 'Exactly one owned native save dialog is required' }
$taskDialog = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$taskDialogs[0])
$taskQueue = [Collections.Generic.Queue[object]]::new()
$taskQueue.Enqueue(@($taskDialog,0))
$taskWalker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
$taskEdits = @()
$taskSaves = @()
$taskCount = 0
$taskDeadline = [DateTime]::UtcNow.AddSeconds(5)
while ($taskQueue.Count -gt 0 -and $taskCount -lt 256 -and [DateTime]::UtcNow -lt $taskDeadline) {
  $taskPair=$taskQueue.Dequeue(); $taskElement=$taskPair[0]; $taskDepth=$taskPair[1]
  $taskCurrent=$taskElement.Current; $taskKind=$taskCurrent.ControlType.ProgrammaticName; $taskCount++
  if ($taskCurrent.ProcessId -ne $OwnerPid) { continue }
  if ($taskCurrent.ClassName -eq 'Edit' -and -not $taskCurrent.IsPassword -and $taskCurrent.AutomationId -eq '1001' -and $taskWalker.GetParent($taskElement).Current.AutomationId -eq 'FileNameControlHost') { $taskEdits += $taskElement }
  if ($taskCurrent.ClassName -eq 'Button' -and $taskCurrent.AutomationId -eq '1') { $taskSaves += $taskElement }
  if ($taskDepth -lt 12 -and $taskKind -notin @('ControlType.List','ControlType.ListItem','ControlType.Tree','ControlType.TreeItem','ControlType.Text','ControlType.Edit')) {
    $taskChild=$taskWalker.GetFirstChild($taskElement)
    while ($null -ne $taskChild -and $taskQueue.Count -lt 256) { $taskQueue.Enqueue(@($taskChild,($taskDepth+1))); $taskChild=$taskWalker.GetNextSibling($taskChild) }
  }
}
if ($taskEdits.Count -ne 1 -or $taskSaves.Count -ne 1) {
  [ArkDockPairDialog]::PostMessage([IntPtr]$taskDialogs[0],0x10,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null
  throw ('Native file name/save controls ambiguous: edits={0}, saveButtons={1}' -f $taskEdits.Count,$taskSaves.Count)
}
# Win32 common-dialog providers may expose only control HWNDs, without UIA
# Value/Invoke patterns. Real input remains scoped to these exact owned controls;
# the filename readback is the only text read and must equal our intended path.
try {
  [ArkDockPairDialog]::TypeFilename([IntPtr]$taskEdits[0].Current.NativeWindowHandle,[IntPtr]$taskDialogs[0],[uint32]$OwnerPid,$taskDestination)
  [ArkDockPairDialog]::Click([IntPtr]$taskSaves[0].Current.NativeWindowHandle,[IntPtr]$taskDialogs[0],[uint32]$OwnerPid)
} catch {
  [ArkDockPairDialog]::PostMessage([IntPtr]$taskDialogs[0],0x10,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null
  throw
}
@{savedDialog=$true;ownerPid=$OwnerPid;filename=[IO.Path]::GetFileName($taskDestination)} | ConvertTo-Json -Compress
