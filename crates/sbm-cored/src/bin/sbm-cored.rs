fn main() {
    if let Err(err) = sbm_cored::serve() {
        eprintln!("sbm-cored: {err}");
        std::process::exit(1);
    }
}
