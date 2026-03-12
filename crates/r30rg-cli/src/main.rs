use clap::{Parser, Subcommand, ValueEnum};
use r30rg_core::chaos::ChaosProfile;
use r30rg_core::scenario::ScenarioContext;
use r30rg_core::types::StackConfig;
use r30rg_scenarios::all_scenarios;
use r30rg_sim::simulator;
use serde_json::json;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "r30rg",
    about = "r30rg — chaos & adversarial testing for Arbitrum rollup stacks",
    long_about = "Deterministic chaos engineering and adversarial testing framework.\n\
                  Same seed = same chaos = reproducible bugs.\n\n\
                  Live mode hits a running nitro-testnode stack via Docker + RPC.\n\
                  Sim mode runs time-compressed deterministic simulations.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Seed for deterministic RNG (0 = random).
    #[arg(long, default_value_t = 1337, global = true)]
    seed: u64,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long, default_value = "info", global = true)]
    log_level: String,

    /// Output format.
    #[arg(long, default_value = "text", global = true, value_enum)]
    output: OutputFormat,
}

#[derive(Clone, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Commands {
    /// List all available scenarios.
    List,

    /// Run a specific scenario against the live stack.
    Run {
        /// Scenario name (or "all" to run everything).
        scenario: String,

        /// Filter by category (e.g. "sequencer-chaos", "invariant-probe").
        #[arg(long)]
        category: Option<String>,

        /// Only run non-destructive scenarios (useful with "all").
        #[arg(long, default_value_t = false)]
        non_destructive: bool,

        /// L1 RPC URL.
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        l1_rpc: String,

        /// L2 RPC URL.
        #[arg(long, default_value = "http://127.0.0.1:8547")]
        l2_rpc: String,

        /// L3 RPC URL (empty = no L3).
        #[arg(long, default_value = "http://127.0.0.1:3347")]
        l3_rpc: String,

        /// Docker compose project name.
        #[arg(long, default_value = "nitro-testnode-live")]
        docker_project: String,
    },

    /// Run deterministic simulation campaign (no live infra needed).
    Sim {
        /// Number of seeds to run.
        #[arg(long, default_value_t = 1000)]
        seeds: u64,

        /// Ticks per seed.
        #[arg(long, default_value_t = 10_000)]
        ticks: u64,
    },

    /// Show chaos profiles.
    Profiles,

    /// Check connectivity to the live stack.
    Ping {
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        l1_rpc: String,

        #[arg(long, default_value = "http://127.0.0.1:8547")]
        l2_rpc: String,

        #[arg(long, default_value = "http://127.0.0.1:3347")]
        l3_rpc: String,

        #[arg(long, default_value = "nitro-testnode-live")]
        docker_project: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(&cli.log_level)),
        )
        .with_target(false)
        .init();

    println!();
    println!("  ┌─────────────────────────────────────────┐");
    println!("  │  r30rg — chaos for Arbitrum rollups      │");
    println!("  │  seed: {:<32} │", cli.seed);
    println!("  └─────────────────────────────────────────┘");
    println!();

    let output_fmt = cli.output.clone();

    match cli.command {
        Commands::List => cmd_list(&output_fmt),
        Commands::Run {
            scenario,
            category,
            non_destructive,
            l1_rpc,
            l2_rpc,
            l3_rpc,
            docker_project,
        } => {
            cmd_run(
                &scenario,
                cli.seed,
                category.as_deref(),
                non_destructive,
                &l1_rpc,
                &l2_rpc,
                &l3_rpc,
                &docker_project,
                &output_fmt,
            )
            .await
        }
        Commands::Sim { seeds, ticks } => cmd_sim(seeds, ticks, cli.seed, &output_fmt),
        Commands::Profiles => cmd_profiles(),
        Commands::Ping {
            l1_rpc,
            l2_rpc,
            l3_rpc,
            docker_project,
        } => cmd_ping(&l1_rpc, &l2_rpc, &l3_rpc, &docker_project).await,
    }
}

fn cmd_list(output_fmt: &OutputFormat) -> anyhow::Result<()> {
    let scenarios = all_scenarios();
    match output_fmt {
        OutputFormat::Json => {
            let items: Vec<_> = scenarios.iter().map(|s| {
                let meta = s.meta();
                let layers: Vec<String> = meta.target_layers.iter().map(|l| l.to_string()).collect();
                json!({
                    "name": meta.name,
                    "category": meta.category.to_string(),
                    "layers": layers,
                    "destructive": meta.destructive,
                    "severity": meta.severity_potential.to_string(),
                    "description": meta.description,
                })
            }).collect();
            println!("{}", serde_json::to_string_pretty(&json!({ "scenarios": items }))?);  
        }
        OutputFormat::Text => {
            println!("  Available scenarios ({}):\n", scenarios.len());
            for s in &scenarios {
                let meta = s.meta();
                let layers: Vec<String> = meta.target_layers.iter().map(|l| l.to_string()).collect();
                println!(
                    "    {:<30} [{:<20}] layers=[{}] destructive={} severity={}",
                    meta.name,
                    meta.category,
                    layers.join(","),
                    meta.destructive,
                    meta.severity_potential
                );
                println!("      {}", meta.description);
                println!();
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn cmd_run(
    scenario_name: &str,
    seed: u64,
    category_filter: Option<&str>,
    non_destructive: bool,
    l1_rpc: &str,
    l2_rpc: &str,
    l3_rpc: &str,
    docker_project: &str,
    output_fmt: &OutputFormat,
) -> anyhow::Result<()> {
    let scenarios = all_scenarios();
    let config = build_config(l1_rpc, l2_rpc, l3_rpc, docker_project);

    let mut to_run: Vec<&dyn r30rg_core::scenario::Scenario> = if scenario_name == "all" {
        scenarios.iter().map(|s| s.as_ref()).collect()
    } else {
        let found = scenarios
            .iter()
            .find(|s| s.meta().name == scenario_name)
            .ok_or_else(|| anyhow::anyhow!("scenario '{}' not found. Use 'r30rg list'.", scenario_name))?;
        vec![found.as_ref()]
    };

    // Apply filters.
    if let Some(cat) = category_filter {
        to_run.retain(|s| s.meta().category.to_string() == cat);
    }
    if non_destructive {
        to_run.retain(|s| !s.meta().destructive);
    }

    if to_run.is_empty() {
        anyhow::bail!("No scenarios matched the given filters.");
    }

    let mut total_pass = 0u32;
    let mut total_fail = 0u32;
    let mut total_err = 0u32;
    let mut json_results: Vec<serde_json::Value> = Vec::new();
    let run_start = std::time::Instant::now();

    for scenario in &to_run {
        let meta = scenario.meta();
        if matches!(output_fmt, OutputFormat::Text) {
            println!("  ▸ Running: {} (seed={})", meta.name, seed);
            println!("    category={} destructive={}", meta.category, meta.destructive);
        }

        let ctx = ScenarioContext::new(seed, config.clone());
        match scenario.execute(ctx).await {
            Ok(outcome) => {
                match &outcome {
                    r30rg_core::types::ScenarioOutcome::Passed { duration_ms, checks_run } => {
                        if matches!(output_fmt, OutputFormat::Text) {
                            println!("    ✓ PASSED ({checks_run} checks in {duration_ms}ms)");
                        }
                        json_results.push(json!({
                            "name": meta.name,
                            "status": "passed",
                            "checks": checks_run,
                            "duration_ms": duration_ms,
                        }));
                        total_pass += 1;
                    }
                    r30rg_core::types::ScenarioOutcome::Failed { duration_ms, findings } => {
                        if matches!(output_fmt, OutputFormat::Text) {
                            println!("    ✗ FAILED ({} findings in {duration_ms}ms)", findings.len());
                            for f in findings {
                                println!("      [{:>8}] {}: {}", f.severity, f.title, f.description);
                            }
                        }
                        let finding_json: Vec<_> = findings.iter().map(|f| json!({
                            "severity": f.severity.to_string(),
                            "title": f.title,
                            "description": f.description,
                        })).collect();
                        json_results.push(json!({
                            "name": meta.name,
                            "status": "failed",
                            "duration_ms": duration_ms,
                            "findings": finding_json,
                        }));
                        total_fail += 1;
                    }
                    r30rg_core::types::ScenarioOutcome::Error { message } => {
                        if matches!(output_fmt, OutputFormat::Text) {
                            println!("    ! ERROR: {message}");
                        }
                        json_results.push(json!({
                            "name": meta.name,
                            "status": "error",
                            "message": message,
                        }));
                        total_err += 1;
                    }
                }
            }
            Err(e) => {
                if matches!(output_fmt, OutputFormat::Text) {
                    println!("    ! ERROR: {e}");
                }
                json_results.push(json!({
                    "name": meta.name,
                    "status": "error",
                    "message": e.to_string(),
                }));
                total_err += 1;
            }
        }
        if matches!(output_fmt, OutputFormat::Text) {
            println!();
        }
    }

    let total_elapsed_ms = run_start.elapsed().as_millis();

    match output_fmt {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&json!({
                "seed": seed,
                "total_elapsed_ms": total_elapsed_ms,
                "passed": total_pass,
                "failed": total_fail,
                "errors": total_err,
                "results": json_results,
            }))?);
        }
        OutputFormat::Text => {
            println!("  ════════════════════════════════════════════");
            println!(
                "  Results: {} passed, {} failed, {} errors ({}ms total)",
                total_pass, total_fail, total_err, total_elapsed_ms
            );
            println!("  ════════════════════════════════════════════");
        }
    }

    if total_fail > 0 || total_err > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_sim(num_seeds: u64, ticks_per_seed: u64, starting_seed: u64, output_fmt: &OutputFormat) -> anyhow::Result<()> {
    if matches!(output_fmt, OutputFormat::Text) {
        println!(
            "  Running deterministic simulation campaign: {} seeds × {} ticks",
            num_seeds, ticks_per_seed
        );
        println!("  Starting seed: {}\n", starting_seed);
    }

    let start = std::time::Instant::now();
    let result = simulator::run_campaign(num_seeds, ticks_per_seed, starting_seed);
    let elapsed = start.elapsed();

    let total_ticks = result.total_ticks;
    let ticks_per_sec = if elapsed.as_secs() > 0 {
        total_ticks / elapsed.as_secs()
    } else {
        total_ticks
    };

    match output_fmt {
        OutputFormat::Json => {
            let violations_json: Vec<_> = result.violations.iter().map(|v| json!({
                "seed": v.seed,
                "tick": v.tick,
                "description": v.description,
            })).collect();
            println!("{}", serde_json::to_string_pretty(&json!({
                "starting_seed": starting_seed,
                "seeds_run": result.seeds_run,
                "seeds_passed": result.seeds_passed,
                "seeds_failed": result.seeds_failed,
                "total_ticks": total_ticks,
                "ticks_per_sec": ticks_per_sec,
                "elapsed_secs": elapsed.as_secs_f64(),
                "violations": violations_json,
            }))?);
        }
        OutputFormat::Text => {
            println!("  ════════════════════════════════════════════");
            println!("  Campaign complete in {:.2}s", elapsed.as_secs_f64());
            println!("  Seeds:  {} run, {} passed, {} failed", result.seeds_run, result.seeds_passed, result.seeds_failed);
            println!("  Ticks:  {} total ({} ticks/sec)", total_ticks, ticks_per_sec);
            if !result.violations.is_empty() {
                println!("\n  Violations ({}):", result.violations.len());
                for v in &result.violations {
                    println!("    seed={} tick={}: {}", v.seed, v.tick, v.description);
                }
            }
            println!("  ════════════════════════════════════════════");
        }
    }

    if result.seeds_failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_profiles() -> anyhow::Result<()> {
    let profiles = vec![
        ("gentle", ChaosProfile::gentle()),
        ("moderate", ChaosProfile::moderate()),
        ("apocalyptic", ChaosProfile::apocalyptic()),
    ];
    println!("  Chaos profiles:\n");
    for (name, p) in &profiles {
        println!("    {name}:");
        println!("      container_pause_rate:     {}", p.container_pause_rate);
        println!("      container_kill_rate:      {}", p.container_kill_rate);
        println!("      network_partition_rate:   {}", p.network_partition_rate);
        println!("      pause_duration:           {}–{}ms", p.pause_duration_min_ms, p.pause_duration_max_ms);
        println!("      partition_duration:       {}–{}ms", p.partition_duration_min_ms, p.partition_duration_max_ms);
        println!("      target_l1:                {}", p.target_l1);
        println!("      target_l3:                {}", p.target_l3);
        println!();
    }
    Ok(())
}

async fn cmd_ping(
    l1_rpc: &str,
    l2_rpc: &str,
    l3_rpc: &str,
    docker_project: &str,
) -> anyhow::Result<()> {
    println!("  Checking connectivity...\n");

    let config = build_config(l1_rpc, l2_rpc, l3_rpc, docker_project);
    let l3_ep = config.l3.as_ref();

    match r30rg_live::rpc::RpcHarness::connect(&config.l1, &config.l2, l3_ep).await {
        Ok(rpc) => {
            let l1b = rpc.block_number(r30rg_core::types::Layer::L1).await?;
            println!("    ✓ L1 ({}) chain_id={} block={}", l1_rpc, rpc.l1.chain_id, l1b);
            let l2b = rpc.block_number(r30rg_core::types::Layer::L2).await?;
            println!("    ✓ L2 ({}) chain_id={} block={}", l2_rpc, rpc.l2.chain_id, l2b);
            if let Some(ref l3) = rpc.l3 {
                let l3b = rpc.block_number(r30rg_core::types::Layer::L3).await?;
                println!("    ✓ L3 ({}) chain_id={} block={}", l3_rpc, l3.chain_id, l3b);
            }
        }
        Err(e) => {
            println!("    ✗ RPC connection failed: {e}");
        }
    }

    match r30rg_live::docker::DockerChaos::connect(docker_project).await {
        Ok(docker) => {
            let services = docker.list_services().await?;
            println!("    ✓ Docker ({}) {} services: {}", docker_project, services.len(), services.join(", "));
        }
        Err(e) => {
            println!("    ✗ Docker connection failed: {e}");
        }
    }

    println!();
    Ok(())
}

fn build_config(l1_rpc: &str, l2_rpc: &str, l3_rpc: &str, docker_project: &str) -> StackConfig {
    let mut config = StackConfig::default();
    config.l1.rpc_url = l1_rpc.to_string();
    config.l2.rpc_url = l2_rpc.to_string();
    config.docker_compose_project = docker_project.to_string();
    if !l3_rpc.is_empty() {
        if let Some(ref mut l3) = config.l3 {
            l3.rpc_url = l3_rpc.to_string();
        }
    } else {
        config.l3 = None;
    }
    config
}
