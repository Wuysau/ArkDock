//! Read only the current process's own main HWND; no dispatch to the WebView thread.
pub(super) fn visible(own_handle: isize) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
        };
        let hwnd = own_handle as windows_sys::Win32::Foundation::HWND;
        let mut process_id = 0;
        unsafe {
            IsWindow(hwnd) != 0
                && GetWindowThreadProcessId(hwnd, &mut process_id) != 0
                && process_id == std::process::id()
                && IsWindowVisible(hwnd) != 0
                && IsIconic(hwnd) == 0
        }
    }
    #[cfg(not(windows))]
    {
        let _ = own_handle;
        false
    }
}
