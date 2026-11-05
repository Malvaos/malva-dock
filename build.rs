//! Compiles the Slint UI (`ui/dock.slint`) into Rust code for `build.rs`.
fn main() {
    slint_build::compile("ui/dock.slint").expect("failed to compile ui/dock.slint");
}
