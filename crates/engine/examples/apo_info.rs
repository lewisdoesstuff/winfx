//! Direct-mode readiness for a capture endpoint.
//!   cargo run -p engine --example apo_info -- [{endpoint-guid}]
fn main() {
    let guid = std::env::args().nth(1).unwrap_or_else(|| "{c950d195-7502-492a-89a4-9d30cbb65fc7}".into());
    match engine::apo::ensure_data_dir() {
        Ok(d) => println!("data dir: {}", d.display()),
        Err(e) => println!("data dir: {e}"),
    }
    println!("installed: {}", engine::apo::installed());
    println!("attached: {}  enhancements off: {}", engine::apo::attached(&guid), engine::apo::enhancements_off(&guid));
    for e in engine::apo::siblings(&guid) {
        println!("  {} state={} attached={} {} / {}", e.guid, e.state, e.attached, e.device, e.name);
    }
}
