//! Embeds `refryt.ico` as the .exe's own icon resource, and the version stamp Windows shows under
//! Properties → Details — Windows only, and specifically the *target* platform, not the host
//! running this build script: cross-compiling for Windows from Linux still needs this to run, so
//! the check is `CARGO_CFG_TARGET_OS` rather than `cfg(windows)`, which would reflect the host
//! instead and never fire at all.
//!
//! The stamp is also what the MSI reads to decide whether to replace an installed `refryt.exe`
//! (`scripts/build-msi.sh` copies it into the File table), so it comes from Cargo's own version
//! rather than being kept by hand. `refryt.rc` is the template; the filled-in copy goes to
//! `OUT_DIR`, with the icon path made absolute so it still resolves from there.

fn main() {
    println!("cargo::rerun-if-changed=refryt.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let var = |name: &str| std::env::var(name).unwrap();
    let manifest_dir = var("CARGO_MANIFEST_DIR").replace('\\', "/");
    let numbers = format!(
        "{},{},{},0",
        var("CARGO_PKG_VERSION_MAJOR"),
        var("CARGO_PKG_VERSION_MINOR"),
        var("CARGO_PKG_VERSION_PATCH"),
    );
    let rc = std::fs::read_to_string("refryt.rc")
        .unwrap()
        .replace("../../", &format!("{manifest_dir}/../../"))
        .replace("@VERSION_NUMBERS@", &numbers)
        .replace("@VERSION@", &var("CARGO_PKG_VERSION"));
    let out = std::path::Path::new(&var("OUT_DIR")).join("refryt.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
