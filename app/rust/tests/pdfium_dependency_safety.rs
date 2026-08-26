#[test]
fn pdfium_render_version_includes_thread_safety_fix() {
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock"))
        .expect("Cargo.lock must be readable");

    assert!(
        lock.contains("name = \"pdfium-render\"\nversion = \"0.9.4\""),
        "pdfium-render must stay on 0.9.4: 0.9.3 can SIGSEGV when RCH concurrently renders PDF pages"
    );
}
