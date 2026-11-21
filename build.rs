fn main() {
    // Embed the app icon into rtidy.exe (shows in Explorer, taskbar, Start menu).
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().expect("embed icon");
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
