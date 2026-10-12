//! Windows: embed the app icon (`motif/brand/rugen.ico`) into rugen.exe.

fn main() {
    println!("cargo:rerun-if-changed=../../motif/brand/rugen.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../motif/brand/rugen.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the icon: {e}");
        }
    }
}
