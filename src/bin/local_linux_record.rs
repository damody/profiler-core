use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::thread;
use std::time::{Duration, Instant};

use mprofiler_proto::mprofiler::RtbStreamRequest;
use profiler_core::grpc::streaming::RtbStreamHandle;
use profiler_core::grpc::sync_control;
use profiler_core::local_linux_record::{
    rtb_csv_header, rtb_csv_row, thread_csv_header, thread_csv_row, LinuxThreadSummary,
};

fn main() -> anyhow::Result<()> {
    let mut addr = "127.0.0.1:50052".to_string();
    let mut seconds = 10.0;
    let mut interval = 1.0;
    let mut pid = 0;
    let mut output = "linux-host-record.csv".to_string();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--addr" => addr = next_value(&mut args, "--addr")?,
            "--seconds" => seconds = next_value(&mut args, "--seconds")?.parse()?,
            "--interval" => interval = next_value(&mut args, "--interval")?.parse()?,
            "--pid" => pid = next_value(&mut args, "--pid")?.parse()?,
            "--out" => output = next_value(&mut args, "--out")?,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    if seconds <= 0.0 {
        anyhow::bail!("--seconds must be > 0");
    }
    if interval <= 0.0 {
        anyhow::bail!("--interval must be > 0");
    }

    let request = RtbStreamRequest {
        pid,
        interval_secs: interval,
        mode: "linux-host".to_string(),
        enable_cpu_loading: true,
        enable_memory: true,
        ..RtbStreamRequest::default()
    };

    let handle = RtbStreamHandle::start_sync(&addr, request, 512)?;
    let mut writer = BufWriter::new(File::create(&output)?);
    writeln!(writer, "{}", rtb_csv_header())?;

    let deadline = Instant::now() + Duration::from_secs_f64(seconds);
    let mut rows = 0usize;
    while Instant::now() < deadline {
        if let Some(point) = handle.poll() {
            writeln!(writer, "{}", rtb_csv_row(&point))?;
            rows += 1;
        } else {
            thread::sleep(Duration::from_millis(50));
        }
    }

    handle.cancel();
    writer.flush()?;
    let thread_output = thread_output_path(&output);
    let thread_count = write_thread_summary(&addr, &thread_output).unwrap_or_else(|err| {
        eprintln!("warning: failed to write thread summary: {err:#}");
        0
    });
    println!("wrote {rows} samples to {output}");
    println!("wrote {thread_count} thread summaries to {thread_output}");
    Ok(())
}

fn next_value(args: &mut impl Iterator<Item = String>, name: &str) -> anyhow::Result<String> {
    args.next()
        .ok_or_else(|| anyhow::anyhow!("{name} requires a value"))
}

fn print_help() {
    println!(
        "Usage: local_linux_record [--addr 127.0.0.1:50052] [--seconds 10] [--interval 1] [--pid 0] [--out linux-host-record.csv]"
    );
}

fn thread_output_path(output: &str) -> String {
    let path = std::path::PathBuf::from(output);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("linux-host-record");
    path.with_file_name(format!("{stem}.threads.csv"))
        .to_string_lossy()
        .into_owned()
}

fn write_thread_summary(addr: &str, output: &str) -> anyhow::Result<usize> {
    let summary = sync_control::get_rtb_summary(addr)?;
    let threads = summary
        .top_threads
        .into_iter()
        .map(|thread| LinuxThreadSummary {
            pid: thread.tid,
            tgid: thread.tgid,
            name: thread.name,
            loading_pct: thread.loading_pct,
            c0_pct: thread.c0_pct,
            c1_pct: thread.c1_pct,
            c2_pct: thread.c2_pct,
            runnable_pct: thread.runnable_pct,
            mips: thread.mips,
            mcps: thread.mcps,
            cpi: thread.cpi,
        })
        .collect::<Vec<_>>();
    let mut writer = BufWriter::new(File::create(output)?);
    writeln!(writer, "{}", thread_csv_header())?;
    for thread in &threads {
        writeln!(writer, "{}", thread_csv_row(thread))?;
    }
    writer.flush()?;
    Ok(threads.len())
}
