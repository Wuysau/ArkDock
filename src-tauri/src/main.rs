#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args_os().skip(1).any(|arg| arg == "--mcp") {
        if let Err(error) = arkdock_core::mcp::run_stdio() {
            eprintln!("ArkDock MCP: {error}");
            std::process::exit(1);
        }
        return;
    }
    arkdock_core::desktop::run();
}
