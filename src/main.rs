use clap::Parser;

fn main() {
    let cli = mars_agents::cli::Cli::parse();
    std::process::exit(mars_agents::cli::dispatch(cli));
}
