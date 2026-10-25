fn main() {
    // Link the Visual C++ runtime statically, as tauri-build does for the app,
    // so pady-cli.exe runs on PCs without the VC++ Redistributable.
    static_vcruntime::metabuild();
}
