#[macro_use]
extern crate afl;
use compiler::compile;

fn main() {
    fuzz!(|data: &[u8]| {
        if let Ok(s) = std::str::from_utf8(data) {
            let _ = compile(s);
        }
    });
}
