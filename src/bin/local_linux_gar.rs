use std::env;

use std::path::PathBuf;

use profiler_core::local_linux_record::{
    generate_linux_host_gar_xlsx, parse_local_linux_csv, parse_thread_summary_csv,
};

fn main() -> anyhow::Result<()> {
    let mut csv = None;
    let mut out = Some("GAR.xlsx".to_string());
    let mut threads = None;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--csv" => csv = Some(next_value(&mut args, "--csv")?),
            "--out" => out = Some(next_value(&mut args, "--out")?),
            "--threads" => threads = Some(next_value(&mut args, "--threads")?),
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    let csv = csv.ok_or_else(|| anyhow::anyhow!("--csv is required"))?;
    let out = out.expect("defaulted");
    let threads = threads.unwrap_or_else(|| default_threads_path(&csv));
    let samples = parse_local_linux_csv(&csv)?;
    let thread_summary = parse_thread_summary_csv(&threads)?;
    generate_linux_host_gar_xlsx(&out, &samples, &thread_summary)?;
    println!(
        "wrote Linux host GAR report with {} samples and {} threads to {}",
        samples.len(),
        thread_summary.len(),
        out
    );
    Ok(())
}

fn next_value(args: &mut impl Iterator<Item = String>, name: &str) -> anyhow::Result<String> {
    args.next()
        .ok_or_else(|| anyhow::anyhow!("{name} requires a value"))
}

fn print_help() {
    println!(
        "Usage: local_linux_gar --csv record.csv [--threads record.threads.csv] [--out GAR.xlsx]"
    );
}

fn default_threads_path(csv: &str) -> String {
    let path = PathBuf::from(csv);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("record");
    path.with_file_name(format!("{stem}.threads.csv"))
        .to_string_lossy()
        .into_owned()
}
