//! Renders a document to a PDF on disk, for looking at.

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("an input file");
    let output = args.next().expect("an output file");
    let source = std::fs::read_to_string(&input).expect("read the input");
    let schema = nib_model::basic::schema();
    let converters = pencil::document::Converters::new(&schema);
    let format = pencil::document::Format::of(std::path::Path::new(&input));
    let doc = converters.parse(format, &source);
    let bytes = pencil::print::to_pdf(&doc, &input, pencil::print::Sheet::default());
    std::fs::write(&output, bytes).expect("write the pdf");
    println!("wrote {output}");
}
