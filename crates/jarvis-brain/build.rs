// sherpa-onnx est lié dynamiquement (la liaison statique casse avec whisper-rs) : les
// bibliothèques sont copiées à côté du binaire par sa crate -sys en développement, et
// dans ~/.local/lib/jarvis/ après `jarvisctl install` (binaire dans ~/.local/bin/).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/..");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib/jarvis");
}
