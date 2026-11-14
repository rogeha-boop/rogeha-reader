//! List a PDF form's fields, or fill some and save a copy.
//! Usage: formfill <in.pdf>                      (list)
//!        formfill <in.pdf> <out.pdf>            (fill the first text fields and tick the first box)
use rogeha_core::{Engine, FieldKind};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let input = args.next().expect("usage: formfill <in.pdf> [out.pdf]");
    let t = std::time::Instant::now();
    let mut engine = Engine::open(input.as_ref()).unwrap();
    let fields = engine.form_fields();
    println!("{} fields on {} pages ({:?})", fields.len(), engine.page_count(), t.elapsed());
    let Some(output) = args.next() else {
        for f in fields.iter().take(40) {
            println!("p{} {:?} {:?} label={:?} value={:?} checked={} ro={} opts={}", f.page + 1, f.kind, f.name, f.label, f.value, f.checked, f.read_only, f.options.len());
        }
        return;
    };
    let mut texts = fields.iter().filter(|f| matches!(f.kind, FieldKind::Text { .. }) && !f.read_only);
    for (field, value) in texts.by_ref().take(3).zip(["Ada Lovelace", "Analytical Engines Ltd", "12 Example Street"]) {
        engine.set_field_value(field.page, field.xref, value).unwrap();
    }
    if let Some(b) = fields.iter().find(|f| f.kind == FieldKind::CheckBox && !f.read_only) {
        engine.set_field_checked(b.page, b.xref, true).unwrap();
    }
    engine.save(Some(output.as_ref())).unwrap();
    println!("saved");
}
