use std::env;
use std::time::Duration;

use profiler_core::grpc::client::{
    ProfilerClient, SourceCpuSelectionOptions, SourceProfileCapabilityOptions,
    SourceProfileStartOptions,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut addr = "http://127.0.0.1:50051".to_string();
    let mut package_name = String::new();
    let mut pid = 0u32;
    let mut duration_ms = 3_000u64;
    let mut sample_period = 1_000u64;
    let mut pmu_buffer_pages = 8_192u32;
    let mut callchain_depth = 16u32;
    let mut enable_spe = true;
    let mut out = "source-profile-bundle.tar.gz".to_string();
    let mut requested_event_keys = Vec::new();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--addr" => addr = next_value(&mut args, "--addr")?,
            "--package" => package_name = next_value(&mut args, "--package")?,
            "--pid" => pid = next_value(&mut args, "--pid")?.parse()?,
            "--duration-ms" => duration_ms = next_value(&mut args, "--duration-ms")?.parse()?,
            "--sample-period" => sample_period = next_value(&mut args, "--sample-period")?.parse()?,
            "--pmu-buffer-pages" => {
                pmu_buffer_pages = next_value(&mut args, "--pmu-buffer-pages")?.parse()?
            }
            "--callchain-depth" => callchain_depth = next_value(&mut args, "--callchain-depth")?.parse()?,
            "--event" => requested_event_keys.push(next_value(&mut args, "--event")?),
            "--no-spe" => enable_spe = false,
            "--out" => out = next_value(&mut args, "--out")?,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    if package_name.trim().is_empty() && pid == 0 {
        anyhow::bail!("--package or --pid is required");
    }
    if duration_ms == 0 {
        anyhow::bail!("--duration-ms must be > 0");
    }
    if sample_period == 0 {
        anyhow::bail!("--sample-period must be > 0");
    }
    if pmu_buffer_pages == 0 {
        anyhow::bail!("--pmu-buffer-pages must be > 0");
    }

    let mut client = ProfilerClient::connect(&addr).await?;
    let cpu_selection = SourceCpuSelectionOptions {
        all_cpus: true,
        ..Default::default()
    };

    let capability = client
        .source_profile_capability(SourceProfileCapabilityOptions {
            package_name: package_name.clone(),
            pid,
            cpu_selection: cpu_selection.clone(),
            enable_pmu: true,
            enable_spe,
            requested_metric_groups: Vec::new(),
        })
        .await?;
    println!(
        "capability: cpus={} warnings={}",
        capability.cpus.len(),
        capability.warnings.len()
    );
    if capability.cpus.is_empty() {
        anyhow::bail!("capability scan returned zero CPU rows");
    }

    let start = client
        .source_profile_start(SourceProfileStartOptions {
            package_name,
            pid,
            cpu_selection,
            enable_pmu: true,
            enable_spe,
            duration_ms,
            pmu_buffer_pages,
            sample_period,
            callchain_depth,
            requested_event_keys,
            ..Default::default()
        })
        .await?;
    if !start.success {
        anyhow::bail!("source profile start failed: {}", start.message);
    }
    println!(
        "started: session={} remote_bundle={}",
        start.session_id, start.remote_bundle_path
    );

    let mut last_remote_bundle = start.remote_bundle_path.clone();
    loop {
        let status = client.source_profile_status(&start.session_id).await?;
        if !status.remote_bundle_path.is_empty() {
            last_remote_bundle = status.remote_bundle_path.clone();
        }
        println!(
            "status: state={} progress={:.1}% samples={} lost={} message={}",
            status.state, status.progress_pct, status.sample_count, status.lost_count, status.message
        );

        match status.state {
            3 => break,
            4 => anyhow::bail!("source profile session failed: {}", status.message),
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }

    let archive = client
        .pull_source_bundle(&last_remote_bundle, &out, |_received, _total| {})
        .await?;
    println!("pulled: remote_archive={} local_archive={}", archive, out);
    Ok(())
}

fn next_value(args: &mut impl Iterator<Item = String>, name: &str) -> anyhow::Result<String> {
    args.next()
        .ok_or_else(|| anyhow::anyhow!("{name} requires a value"))
}

fn print_help() {
    println!(
        "Usage: source_profile_smoke --package <pkg>|--pid <pid> [--addr http://127.0.0.1:50051] [--duration-ms 3000] [--sample-period 1000] [--pmu-buffer-pages 8192] [--callchain-depth 16] [--event cpu_cycles] [--no-spe] [--out bundle.tar.gz]"
    );
}
