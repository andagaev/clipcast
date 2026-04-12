mod duration;

fn main() {
    let _ = duration::parse;
    println!("clipcast v{}", env!("CARGO_PKG_VERSION"));
}
