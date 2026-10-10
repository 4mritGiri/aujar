mod convert;
mod picker;

pub use convert::Rgb;
pub use picker::pick;

pub fn rgb_to_hex(r: u8, g: u8, b: u8) -> String {
    Rgb::new(r, g, b).to_hex()
}
