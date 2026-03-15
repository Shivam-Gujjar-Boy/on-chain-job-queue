mod sim;

use anyhow::Result;
use clap::Parser;
use sim::chain::ChainClient;
use sim::config::SimulationConfig;
use sim::engine::{build_initial_snapshot, run_simulation};
use sim::report::export_results;
use sim::types::{SharedState, SimulationSnapshot};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[derive(Parser, Debug)]
#[command(name = "jq-sim")]
#[command(about = "Localnet real-time simulator + TUI for on-chain job queue")]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,

    #[arg(long, default_value = "~/.config/solana/id.json")]
    keypair: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = SimulationConfig::prompt(cli.rpc_url.clone(), cli.keypair.clone())?;

    println!("\nUsing RPC: {}", config.rpc_url);
    println!("Payer keypair: {}", config.keypair_path);
    println!("Starting simulation setup...\n");

    let chain = Arc::new(ChainClient::new(&config.rpc_url, &config.keypair_path)?);
    println!("Payer: {}", chain.payer_pubkey());

    chain.preflight_checks()?;

    let initial_snapshot: SimulationSnapshot = build_initial_snapshot(&config);
    let state: SharedState = Arc::new(Mutex::new(initial_snapshot));
    let stop = Arc::new(AtomicBool::new(false));

    let sim_state = state.clone();
    let sim_chain = chain.clone();
    let sim_stop = stop.clone();
    let sim_config = config.clone();

    let simulation_handle = tokio::spawn(async move {
        if let Err(err) = run_simulation(sim_config, sim_chain, sim_state.clone(), sim_stop.clone()).await {
            eprintln!("Simulation initialization/runtime error: {err}");
            if let Ok(mut guard) = sim_state.lock() {
                guard.push_log(format!("simulation error: {err}"));
                guard.done = true;
            }
        }
    });

    let ui_state = state.clone();
    let ui_stop = stop.clone();
    let ui_handle = tokio::task::spawn_blocking(move || sim::ui::run_tui(ui_state, ui_stop));

    ui_handle.await??;
    let _ = simulation_handle.await;

    if let Ok(guard) = state.lock() {
        if guard.total_job_submissions == 0 && !guard.recent_logs.is_empty() {
            println!("\nRecent simulator logs:");
            for line in guard.recent_logs.iter().rev().take(8).rev() {
                println!("  - {}", line);
            }
        }
    }

    let snapshot = state
        .lock()
        .map(|guard| guard.clone())
        .map_err(|_| anyhow::anyhow!("failed to acquire simulation state for result export"))?;

    let (txt_path, json_path) = export_results(&snapshot, &config)?;
    println!("\nSaved simulation results:");
    println!("  - {}", txt_path.display());
    println!("  - {}", json_path.display());

    println!("Simulation terminated.");
    Ok(())
}
