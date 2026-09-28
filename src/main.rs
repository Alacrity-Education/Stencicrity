//! stencicrity - merge KiCad paste gerbers into one stencil order. See README.md.
fn main() {
    std::process::exit(stencicrity::cli::main(std::env::args().collect()));
}
