use std::fs;
use std::path::Path;
use markdown;

fn main() {
    let input_dir = Path::new("static");
    let output_dir = Path::new("public");

    fs::create_dir_all(&output_dir).unwrap();

    for entry in fs::read_dir(input_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().unwrap().is_file())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
    {
        let path = entry.path();
        let file_name = path.file_stem().unwrap().to_string_lossy();
        let out_path = output_dir.join(format!("{file_name}.html"));

        let md = fs::read_to_string(path).unwrap();
        let html = markdown::to_html(&md);

        fs::write(out_path, html).unwrap();
    }
}
