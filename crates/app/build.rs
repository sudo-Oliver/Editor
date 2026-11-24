//! Puts the icon and the version into the program file on Windows, so that
//! Explorer, the taskbar and the Start menu show them.

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/windows/AppIcon.ico");
    // Resource compilers come with Windows' own tools: a build from another
    // system, as a check on the Mac, goes without the icon.
    let target_windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    if !target_windows || !cfg!(windows) {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("../../packaging/windows/AppIcon.ico");
    resource.set("ProductName", "Editor");
    resource.set("FileDescription", "Editor");
    if let Err(error) = resource.compile() {
        println!("cargo:warning=The program has no icon: {error}");
    }
}
