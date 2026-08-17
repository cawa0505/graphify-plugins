fn main() {
    let path = std::env::args().nth(1).expect("Usage: tskeleton <file>");
    match graphify_plugin_skeleton::extract_skeleton(&path) {
        Ok(s) => println!("{}", s),
        Err(e) => eprintln!("Error: {}", e),
    }
}