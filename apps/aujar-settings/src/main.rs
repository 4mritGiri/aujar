fn main() {
    println!("Aujar Settings");
    println!(
        "Configuration path: {}",
        aujar_config::Config::default_path().display()
    );
}
