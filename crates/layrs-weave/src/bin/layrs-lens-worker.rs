fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    if std::env::args_os().skip(1).collect::<Vec<_>>() == ["--wasm-v1"] {
        if let Err(error) =
            layrs_weave::wasm_host::serve(std::io::stdin().lock(), std::io::stdout().lock())
        {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args_os().skip(1).collect::<Vec<_>>() != ["--protocol-v1"] {
        std::process::exit(64);
    }
    if let Err(error) =
        layrs_weave::worker::serve(std::io::stdin().lock(), std::io::stdout().lock())
    {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
