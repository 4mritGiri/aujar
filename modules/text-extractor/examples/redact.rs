//! Try Auto-Redact on a picture: `cargo run -p aujar-text-extractor --example redact -- in.png out.png`.

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (input, output) = (args.next().expect("input"), args.next().expect("output"));
    let picture = image::open(&input)?.to_rgba8();
    let (covered, count) =
        aujar_text_extractor::redact_image(&picture, &[], aujar_text_extractor::Engine::Auto)?;

    covered.save(&output)?;
    println!("covered {count} area(s) -> {output}");

    Ok(())
}
