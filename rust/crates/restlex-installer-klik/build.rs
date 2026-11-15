#[cfg(windows)]
fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon(r"..\..\..\icon.ico");
    res.set("FileDescription", "RestLex Installer");
    res.set("ProductName",     "RestLex");
    res.set_manifest(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{35138b9a-5d96-4fbe-8c99-9c43c8571bf2}"/>
      <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/>
    </application>
  </compatibility>
</assembly>"#);
    if let Err(e) = res.compile() {
        eprintln!("warning: winres: {}", e);
    }
    println!("cargo:rerun-if-changed=../../../icon.ico");
}

#[cfg(not(windows))]
fn main() {}
