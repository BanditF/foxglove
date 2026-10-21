#[cfg(windows)]
fn main() {
    winresource::WindowsResource::new()
        .set_icon("resources/icon.ico")
        .compile()
        .expect("Failed to embed Windows resources.");
}

#[cfg(not(windows))]
fn main() {}
