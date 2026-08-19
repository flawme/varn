use clap::Parser;
use varn::cli;

fn main() {
    let cli = cli::Cli::parse();
    let json = cli.json;
    if let Err(e) = cli::run(cli) {
        if json {
            let output = serde_json::json!({
                "status": "error",
                "error": e.to_string(),
            });
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&output)
                    .unwrap_or_else(|_| r#"{"status":"error"}"#.to_string())
            );
        } else {
            eprintln!("Error: {e}");
        }
        std::process::exit(1);
    }
}
