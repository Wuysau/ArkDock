//! Explicit local diagnostic. Reads only ArkDock's selection subdirectory.
use arkdock_core::selection::SelectionStore;
use std::path::PathBuf;

fn main() {
    if !cfg!(windows) {
        eprintln!("BLOCKED WINDOWS_REQUIRED");
        std::process::exit(2);
    }
    let Some(app_data) = std::env::var_os("APPDATA") else {
        eprintln!("BLOCKED APPDATA_REQUIRED");
        std::process::exit(2);
    };
    let app_data = PathBuf::from(app_data).join("dev.arkdock.desktop");
    let selection = app_data.join("selection-context-v1");
    println!("appDataCanonical={:?}", std::fs::canonicalize(&app_data));
    println!("selectionCanonical={:?}", std::fs::canonicalize(&selection));
    println!(
        "selectionMetadata={:?}",
        std::fs::symlink_metadata(&selection)
            .map(|metadata| (metadata.is_dir(), metadata.file_type().is_symlink()))
    );
    match SelectionStore::open(&app_data) {
        Ok(_) => println!("PASS SELECTION_STORE_OPEN"),
        Err(error) => {
            eprintln!("FAIL {error}");
            std::process::exit(1);
        }
    }
}
