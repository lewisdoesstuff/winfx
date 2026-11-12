//! Show how a preset file imports:  cargo run -p engine --example import -- <file>
use engine::import::{Imported, import};

fn main() {
    let path = std::env::args().nth(1).expect("usage: import <preset.json | ParametricEQ.txt>");
    let text = std::fs::read_to_string(&path).expect("read file");
    match import(&text) {
        Ok(Imported::Chain(chain, r)) => {
            println!("EasyEffects chain:");
            for s in &chain.slots {
                println!("  {}{}  {:?}", s.effect.name(), if s.bypass { " (bypassed)" } else { "" }, s.effect);
            }
            println!("mapped: {:?}\napproximated: {:?}\nskipped: {:?}", r.mapped, r.approximated, r.skipped);
        }
        Ok(Imported::Eq(eq, r)) => {
            println!("EQ, {} bands, gain {:+.1} dB", eq.bands.len(), eq.gain_db);
            println!("mapped: {:?}\napproximated: {:?}\nskipped: {:?}", r.mapped, r.approximated, r.skipped);
        }
        Err(e) => println!("error: {e}"),
    }
}
