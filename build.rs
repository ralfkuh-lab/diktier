//! Build-Script (Phase 5, Paket F): hängt Icon und Versionsressource an die
//! Exe.
//!
//! Nur Windows — `cfg(windows)` ist hier der **Host**, und genau der stimmt:
//! Die Ressource entsteht beim Bauen über `rc.exe` aus dem Windows SDK. Unter
//! Linux ist `main` leer und `winresource` steht gar nicht erst im Baum
//! (`[target.'cfg(windows)'.build-dependencies]`).
//!
//! Ein Fehlschlag ist **nicht** fatal: fehlt `rc.exe`, bekommt die Exe eben das
//! Standardicon. Der Installer nimmt `assets/diktier.ico` ohnehin direkt.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/diktier.ico");
        println!("cargo:rerun-if-changed=build.rs");

        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/diktier.ico");
        res.set("ProductName", "Diktier");
        res.set(
            "FileDescription",
            "Diktier — lokales Push-to-Talk-Diktiertool",
        );
        res.set("CompanyName", "Ralf Kuhlendahl");
        res.set("LegalCopyright", "MIT-Lizenz");
        if let Err(err) = res.compile() {
            println!("cargo:warning=Exe-Ressource nicht erzeugt ({err}) — Exe bleibt ohne Icon");
        }
    }
}
