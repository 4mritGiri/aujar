pub fn distance(a: (i32, i32), b: (i32, i32)) -> f64 {
    let dx = (b.0 - a.0) as f64;
    let dy = (b.1 - a.1) as f64;
    (dx * dx + dy * dy).sqrt()
}
