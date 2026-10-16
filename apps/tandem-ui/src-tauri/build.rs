fn main() {
    // Sin la feature `app` no hay app de Tauri que preparar.
    #[cfg(feature = "app")]
    tauri_build::build();
}
