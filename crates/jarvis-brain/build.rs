// sherpa-onnx est lié dynamiquement (la liaison statique casse avec whisper-rs) : les
// bibliothèques sont copiées à côté du binaire par sa crate -sys, on les y cherche.
fn main() {
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/..");
}
