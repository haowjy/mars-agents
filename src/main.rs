use clap::Parser;

fn main() {
    let cli = mars_agents::cli::Cli::try_parse().unwrap_or_else(|error| {
        let args = std::env::args().collect::<Vec<_>>();
        if let Some(hint) = removed_models_list_flag_hint(&args) {
            eprint!("{error}");
            eprintln!("hint: {hint}");
            std::process::exit(error.exit_code());
        }
        error.exit();
    });
    std::process::exit(mars_agents::cli::dispatch(cli));
}

/// These are errors, not compatibility flags: clap still rejects each old arg.
fn removed_models_list_flag_hint(args: &[String]) -> Option<&'static str> {
    let list = args.windows(2).any(|pair| pair == ["models", "list"]);
    if !list {
        return None;
    }
    let flag = args.iter().find_map(|arg| {
        [
            "--include",
            "--exclude",
            "--providers",
            "--no-visibility",
            "--catalog",
            "--unavailable",
        ]
        .into_iter()
        .find(|flag| *arg == *flag || arg.starts_with(&format!("{flag}=")))
    })?;
    Some(match flag {
        "--catalog" => "use `mars models catalog` for raw models.dev entries",
        "--no-visibility" => "use `mars models list --all` to include hidden curated rows",
        "--unavailable" => "use `mars models list --all --live` to inspect eligibility",
        _ => "move display filters to mars.curated.toml; see `mars models list --help`",
    })
}
