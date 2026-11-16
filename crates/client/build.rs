// Embeds assets/icon.ico in r5rust.exe so Explorer, pinned shortcuts and the taskbar show the app icon.
fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not embedded: {e}");
        }
    }
}
