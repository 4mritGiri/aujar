fn main() {
    println!("Nujar Settings");
    println!(
        "Configuration path: {}",
        aujar_config::Config::default_path().display()
    );
}
