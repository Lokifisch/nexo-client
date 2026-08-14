//! Stamps the Windows executable with its icon and version information.
//!
//! Without this, `nexo.exe` gets the generic Windows application icon and its
//! Properties dialog is blank — and the Inno Setup script, which reads its
//! `AppVersion` straight out of the built binary, would have nothing to read.
//! That indirection is deliberate: it keeps the version in exactly one place
//! (`Cargo.toml`) rather than repeated in a packaging script that would drift.

fn main() {
    println!("cargo::rerun-if-changed=../../assets/nexo.ico");

    // The *target* OS, not `cfg!(windows)`. A build script runs on the host,
    // so `cfg!(windows)` is false when cross-compiling from Linux to Windows
    // and the resource would be silently skipped — producing an .exe with no
    // icon and no version info, from a build that reported success.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("../../assets/nexo.ico");
    resource.set("ProductName", "Nexo");
    resource.set("FileDescription", "Nexo — native Minecraft client");
    resource.set("CompanyName", "Lokifisch");
    resource.set("LegalCopyright", "MIT licensed. See LICENSE.");
    resource.set("OriginalFilename", "nexo.exe");

    if let Err(err) = resource.compile() {
        // Failing the build is right: a Windows release that quietly lost its
        // icon and version block would only be noticed after publishing, and
        // the installer reads its version from here.
        panic!("could not embed the Windows resources: {err}");
    }
}
