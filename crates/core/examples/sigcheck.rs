//! Print the digital signatures of a PDF and what the reader makes of them.
//! Usage: sigcheck <file.pdf>
fn main() {
    let path = std::env::args_os().nth(1).expect("usage: sigcheck <file.pdf>");
    let engine = rogeha_core::Engine::open(path.as_ref()).unwrap();
    let sigs = engine.signatures();
    if sigs.is_empty() {
        println!("no signatures");
    }
    for s in sigs {
        println!(
            "{}: {:?} | signer: {} | whole file: {} | self-signed: {} | cert valid then: {} | at: {}",
            s.field,
            s.status,
            s.signer,
            s.covers_whole_file,
            s.self_signed,
            s.certificate_valid_at_signing,
            s.signed_at.map(rogeha_core::format_time).unwrap_or_default()
        );
    }
}
