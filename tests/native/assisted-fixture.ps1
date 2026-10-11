# Owned GUI fixture only. It is not an Agent and cannot enable product support.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName PresentationFramework
Add-Type -AssemblyName PresentationCore
Add-Type -AssemblyName WindowsBase
Add-Type -ReferencedAssemblies @('PresentationFramework','PresentationCore','WindowsBase','System.Xaml') @'
using System;
using System.Collections.Concurrent;
using System.Threading;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Threading;
public static class ArkDockAssistedFixture {
  static Window main, other;
  static StackPanel panel;
  static TextBox composer;
  static PasswordBox password;
  static Button submit;
  static int submissions, enters;
  static ConcurrentQueue<string> commands = new ConcurrentQueue<string>();
  static void NewComposer() {
    if (composer != null) panel.Children.Remove(composer);
    if (password != null) { panel.Children.Remove(password); password = null; }
    composer = new TextBox { Height = 180, Width = 470, AcceptsReturn = true, TextWrapping = TextWrapping.Wrap };
    AutomationProperties.SetAutomationId(composer, "ArkDockFixtureComposer");
    AutomationProperties.SetName(composer, "ArkDock owned test composer");
    composer.PreviewKeyDown += (sender, args) => { if (args.Key == Key.Enter) enters++; };
    panel.Children.Insert(0, composer);
  }
  static void Report(string op) {
    string encoded = Convert.ToBase64String(System.Text.Encoding.UTF8.GetBytes(composer.Text));
    Console.WriteLine("{\"op\":\"" + op + "\",\"pid\":" + System.Diagnostics.Process.GetCurrentProcess().Id + ",\"first\":" + new WindowInteropHelper(main).Handle.ToInt64() + ",\"second\":" + new WindowInteropHelper(other).Handle.ToInt64() + ",\"valueBase64\":\"" + encoded + "\",\"submissions\":" + submissions + ",\"enterEvents\":" + enters + "}");
    Console.Out.Flush();
  }
  public static void Run() {
    var app = new Application();
    app.ShutdownMode = ShutdownMode.OnMainWindowClose;
    main = new Window { Title = "ArkDock assisted owned fixture (not an Agent)", Width = 530, Height = 340, Left = 60, Top = 80, Topmost = true };
    other = new Window { Title = "ArkDock assisted second owned fixture", Width = 300, Height = 170, Left = 620, Top = 80 };
    panel = new StackPanel { Margin = new Thickness(20) };
    main.Content = panel;
    NewComposer();
    submit = new Button { Content = "User confirms and submits", Margin = new Thickness(0,16,0,0) };
    submit.Click += (sender, args) => { submissions++; };
    panel.Children.Add(submit);
    other.Content = new TextBox { Text = "Unrelated fixture input; must remain unchanged", Margin = new Thickness(12) };
    other.Show(); main.Show(); main.Activate(); composer.Focus();
    var input = new Thread(() => { string line; while ((line = Console.ReadLine()) != null) commands.Enqueue(line); });
    input.IsBackground = true; input.Start();
    var timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(20) };
    timer.Tick += (sender, args) => {
      string command;
      while (commands.TryDequeue(out command)) {
        switch (command) {
          case "focus": main.Activate(); composer.Focus(); break;
          case "switch": other.Activate(); break;
          case "reset": composer.Text = ""; main.Activate(); composer.Focus(); break;
          case "draft": composer.Text = "user draft must be preserved"; break;
          case "replace": NewComposer(); composer.Focus(); break;
          case "password":
            panel.Children.Remove(composer);
            password = new PasswordBox { Width = 470, Height = 40 };
            AutomationProperties.SetAutomationId(password, "ArkDockFixtureComposer");
            AutomationProperties.SetName(password, "ArkDock owned test composer");
            panel.Children.Insert(0, password);
            break;
          case "readonly": composer.IsReadOnly = true; break;
          case "editable": composer.IsReadOnly = false; break;
          case "user_submit": submit.RaiseEvent(new RoutedEventArgs(Button.ClickEvent)); break;
          case "close": other.Close(); main.Close(); return;
        }
        Report(command);
      }
    };
    timer.Start(); Report("ready"); app.Run(main);
  }
}
'@
[ArkDockAssistedFixture]::Run()
