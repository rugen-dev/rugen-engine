//! `cargo run -p rugen-fair --example profile -- path/to/char.def [data_dir]`:
//! fingerprint, canonical files and fairness profile of a character.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(def) = args.first() else {
        eprintln!("usage: profile <char.def> [data_dir]");
        std::process::exit(2);
    };
    let data = args.get(1).map(std::path::PathBuf::from);
    let files = match rugen_fair::files::char_files(def.as_ref(), data.as_deref()) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let fp = rugen_fair::fingerprint(&files).unwrap_or_default();
    println!("fingerprint {fp}");
    println!(
        "{} files, {} bytes",
        files.entries.len(),
        files.total_size()
    );
    for e in &files.entries {
        println!("  {}", e.path);
    }
    match rugen_fair::profile::analyze(&files) {
        Ok(p) => println!("{p:#?}"),
        Err(e) => eprintln!("profile: {e}"),
    }
}
