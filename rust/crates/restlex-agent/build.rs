#[cfg(windows)]
fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon(r"..\..\..\icon.ico");
    res.set("FileDescription", "Restlex Backup Agent (Windows Service)");
    res.set("ProductName", "Restlex Agent");
    if let Err(e) = res.compile() {
        eprintln!("warning: failed to embed Windows resources: {}", e);
    }
    println!("cargo:rerun-if-changed=../../../icon.ico");
}

#[cfg(not(windows))]
fn main() {}
