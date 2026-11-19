fn main() {
    // Without the `app` feature there is no Tauri app to prepare.
    #[cfg(feature = "app")]
    tauri_build::build();
}
