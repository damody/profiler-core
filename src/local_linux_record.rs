use std::path::Path;

use crate::proto::RtbDataPoint;

#[derive(Debug, Clone, PartialEq)]
pub struct LinuxHostSample {
    pub timestamp_ms: u64,
    pub process_name: String,
    pub cpu_usage_pct: f64,
    pub bcpu_usage_pct: f64,
    pub mem_total_kb: u64,
    pub mem_available_kb: u64,
    pub top_app_rss_kb: u64,
    pub total_mips: f64,
    pub total_mcps: f64,
    pub cpi: f64,
    pub cache_references_per_sec: f64,
    pub cache_misses_per_sec: f64,
    pub cache_miss_rate_pct: f64,
    pub branch_instructions_per_sec: f64,
    pub branch_misses_per_sec: f64,
    pub branch_miss_rate_pct: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinuxThreadSummary {
    pub pid: i32,
    pub tgid: i32,
    pub name: String,
    pub loading_pct: f64,
    pub c0_pct: f64,
    pub c1_pct: f64,
    pub c2_pct: f64,
    pub runnable_pct: f64,
    pub mips: f64,
    pub mcps: f64,
    pub cpi: f64,
}

pub fn rtb_csv_header() -> &'static str {
    "timestamp_ms,process_name,cpu_usage_pct,bcpu_usage_pct,mem_total_kb,mem_available_kb,top_app_rss_kb,total_mips,total_mcps,cpi,cache_references_per_sec,cache_misses_per_sec,cache_miss_rate_pct,branch_instructions_per_sec,branch_misses_per_sec,branch_miss_rate_pct"
}

pub fn rtb_csv_row(point: &RtbDataPoint) -> String {
    let cpu_usage = point.cpu_usages_pct.first().copied().unwrap_or(0.0);
    format!(
        "{},{},{:.3},{:.3},{},{},{},{:.6},{:.6},{:.6},{:.3},{:.3},{:.6},{:.3},{:.3},{:.6}",
        point.timestamp_ms,
        csv_escape(&point.process_name),
        cpu_usage,
        point.bcpu_usage_pct,
        point.mem_total_kb,
        point.mem_available_kb,
        point.top_app_rss_kb,
        point.total_mips,
        point.total_mcps,
        point.cpi,
        point.cache_references_per_sec,
        point.cache_misses_per_sec,
        point.cache_miss_rate_pct,
        point.branch_instructions_per_sec,
        point.branch_misses_per_sec,
        point.branch_miss_rate_pct
    )
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub fn parse_local_linux_csv(path: impl AsRef<Path>) -> anyhow::Result<Vec<LinuxHostSample>> {
    let content = std::fs::read_to_string(path)?;
    let mut samples = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        if line_index == 0 || line.trim().is_empty() {
            continue;
        }
        let fields = parse_csv_line(line);
        if fields.len() != 7 && fields.len() != 16 {
            anyhow::bail!(
                "invalid local linux csv row {}: expected 7 or 16 fields",
                line_index + 1
            );
        }
        samples.push(LinuxHostSample {
            timestamp_ms: fields[0].parse()?,
            process_name: fields[1].clone(),
            cpu_usage_pct: fields[2].parse()?,
            bcpu_usage_pct: fields[3].parse()?,
            mem_total_kb: fields[4].parse()?,
            mem_available_kb: fields[5].parse()?,
            top_app_rss_kb: fields[6].parse()?,
            total_mips: parse_optional_f64(&fields, 7)?,
            total_mcps: parse_optional_f64(&fields, 8)?,
            cpi: parse_optional_f64(&fields, 9)?,
            cache_references_per_sec: parse_optional_f64(&fields, 10)?,
            cache_misses_per_sec: parse_optional_f64(&fields, 11)?,
            cache_miss_rate_pct: parse_optional_f64(&fields, 12)?,
            branch_instructions_per_sec: parse_optional_f64(&fields, 13)?,
            branch_misses_per_sec: parse_optional_f64(&fields, 14)?,
            branch_miss_rate_pct: parse_optional_f64(&fields, 15)?,
        });
    }
    Ok(samples)
}

pub fn generate_linux_host_gar_xlsx(
    path: impl AsRef<Path>,
    samples: &[LinuxHostSample],
    threads: &[LinuxThreadSummary],
) -> anyhow::Result<()> {
    let path = path.as_ref();
    let mut book = umya_spreadsheet::new_file();
    book.get_sheet_by_name_mut("Sheet1")
        .expect("new workbook has Sheet1")
        .set_name("summary");

    write_summary_sheet(&mut book, samples, threads)?;
    write_advanced_report_sheet(&mut book, samples)?;
    write_summary_vertical_sheet(&mut book, samples)?;
    write_gpt_summary_sheet(&mut book, samples)?;
    write_raw_data_sheet(&mut book, samples)?;
    write_power_summary_sheet(&mut book, samples)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    umya_spreadsheet::writer::xlsx::write(&book, path)?;
    Ok(())
}

pub fn thread_csv_header() -> &'static str {
    "pid,tgid,name,loading_pct,c0_pct,c1_pct,c2_pct,runnable_pct,mips,mcps,cpi"
}

pub fn thread_csv_row(thread: &LinuxThreadSummary) -> String {
    format!(
        "{},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
        thread.pid,
        thread.tgid,
        csv_escape(&thread.name),
        thread.loading_pct,
        thread.c0_pct,
        thread.c1_pct,
        thread.c2_pct,
        thread.runnable_pct,
        thread.mips,
        thread.mcps,
        thread.cpi
    )
}

pub fn parse_thread_summary_csv(path: impl AsRef<Path>) -> anyhow::Result<Vec<LinuxThreadSummary>> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)?;
    let mut threads = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        if line_index == 0 || line.trim().is_empty() {
            continue;
        }
        let fields = parse_csv_line(line);
        if fields.len() != 11 {
            anyhow::bail!(
                "invalid thread csv row {}: expected 11 fields",
                line_index + 1
            );
        }
        threads.push(LinuxThreadSummary {
            pid: fields[0].parse()?,
            tgid: fields[1].parse()?,
            name: fields[2].clone(),
            loading_pct: fields[3].parse()?,
            c0_pct: fields[4].parse()?,
            c1_pct: fields[5].parse()?,
            c2_pct: fields[6].parse()?,
            runnable_pct: fields[7].parse()?,
            mips: fields[8].parse()?,
            mcps: fields[9].parse()?,
            cpi: fields[10].parse()?,
        });
    }
    Ok(threads)
}

fn write_summary_sheet(
    book: &mut umya_spreadsheet::Spreadsheet,
    samples: &[LinuxHostSample],
    threads: &[LinuxThreadSummary],
) -> anyhow::Result<()> {
    let ws = book
        .get_sheet_by_name_mut("summary")
        .ok_or_else(|| anyhow::anyhow!("missing summary sheet"))?;

    let top_headers = [
        "FPS Avg",
        "FPS Min",
        "FPS Target FPS",
        "Perf index All",
        "Perf index B",
        "Perf index M",
        "FPS CPU Time",
        "FPS GPU Time",
    ];
    write_row_strings(ws, 2, &top_headers);
    write_row_numbers(ws, 3, &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);

    let cpu_gpu_headers = [
        "CPU C0 Freq",
        "CPU_MCUPM C0 Freq",
        "CPU C0 Loading",
        "CPU C1 Freq",
        "CPU_MCUPM C1 Freq",
        "CPU C1 Loading",
        "CPU C2 Freq",
        "CPU_MCUPM C2 Freq",
        "CPU C2 Loading",
        "GPU Freq",
        "GPU Urate",
        "Dram Freq",
        "DSU Freq(MHz)",
        "EMI Throughput BW Total",
        "EMI Throughput BW CPU",
        "EMI Throughput BW GPU",
        "EMI Throughput BW MM",
    ];
    write_row_strings(ws, 5, &cpu_gpu_headers);
    write_row_numbers(
        ws,
        6,
        &[
            0.0,
            0.0,
            avg_cpu(samples),
            0.0,
            0.0,
            avg_cpu(samples),
            0.0,
            0.0,
            avg_cpu(samples),
            avg_mips(samples),
            0.0,
            avg_mips(samples),
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        ],
    );

    let metric_headers = [
        "Expected Fpks",
        "Avg Frame Ctrl Timer Interval",
        "Is Pre-throttle",
        "Estimated CPU power (mA) (Beta)",
        "Estimated DSU power (mA) (Beta)",
        "Game MIPS",
        "System MIPS",
        "Total MIPS",
        "1% Low",
        "Board Temp Avg (C)",
        "Battery Temp Avg (C)",
        "Start Temp (C)",
        "End Temp (C)",
    ];
    write_row_strings(ws, 8, &metric_headers);
    write_row_numbers(ws, 9, &[0.0; 13]);

    let task_headers = [
        "pid",
        "tgid",
        "name",
        "Loading(%)",
        "C0(%)",
        "C1(%)",
        "C2(%)",
        "runnable(%)",
        "MIPS",
        "MCPS",
        "CPI",
    ];
    write_row_strings(ws, 22, &task_headers);
    for (index, thread) in threads.iter().take(20).enumerate() {
        let row = 23 + index as u32;
        set_number(ws, row, 1, thread.pid as f64);
        set_number(ws, row, 2, thread.tgid as f64);
        set_text(ws, row, 3, &thread.name);
        set_number(ws, row, 4, round2(thread.loading_pct));
        set_number(ws, row, 5, round2(thread.c0_pct));
        set_number(ws, row, 6, round2(thread.c1_pct));
        set_number(ws, row, 7, round2(thread.c2_pct));
        set_number(ws, row, 8, round2(thread.runnable_pct));
        set_number(ws, row, 9, round2(thread.mips));
        set_number(ws, row, 10, round2(thread.mcps));
        set_number(ws, row, 11, round2(thread.cpi));
    }
    Ok(())
}

fn write_advanced_report_sheet(
    book: &mut umya_spreadsheet::Spreadsheet,
    samples: &[LinuxHostSample],
) -> anyhow::Result<()> {
    add_sheet(book, "Advanced Report")?;
    let ws = book
        .get_sheet_by_name_mut("Advanced Report")
        .ok_or_else(|| anyhow::anyhow!("missing Advanced Report sheet"))?;

    set_text(ws, 1, 1, "System Index*");
    set_text(ws, 1, 2, "Key Index");
    set_text(ws, 1, 3, "Linux Host Local");

    let rows: Vec<(&str, &str, Option<String>)> = vec![
        ("performance/power", "Avg FPS", Some("0.00".into())),
        ("", "Avg Power (mA)", Some("0.00".into())),
        ("", "1% Low", Some("0.00".into())),
        (
            "MIPS",
            "Total MIPS",
            Some(format!("{:.0}", avg_mips(samples))),
        ),
        ("", "Game MIPS", Some(format!("{:.0}", avg_mips(samples)))),
        ("", "- Logical MIPS", None),
        ("", "- Render MIPS", None),
        ("", "System MIPS", Some(format!("{:.0}", avg_mips(samples)))),
        ("BW", "Total EMI Thr. (MB/s)", None),
        ("", "CPU EMI Thr. (MB/s)", None),
        ("", "GPU EMI Thr. (MB/s)", None),
        (
            "Game Task Info",
            "Logical Thread CPU Usage (BCPU/MCPU)",
            None,
        ),
        ("", "Logical Loading", None),
        ("", "Logical CPI", None),
        ("", "Render Thread CPU Usage (BCPU/MCPU)", None),
        ("", "Render Loading", None),
        ("", "Render CPI", None),
        ("System Indices", "Avg. BCPU Freq. (MHz)", Some("0".into())),
        (
            "",
            "Avg. BCPU Usage (%)",
            Some(format!("{:.1}%", avg_cpu(samples))),
        ),
        ("", "Avg. MCPU Freq. (MHz)", Some("0".into())),
        (
            "",
            "Avg. MCPU Usage (%)",
            Some(format!("{:.1}%", avg_cpu(samples))),
        ),
        ("", "Avg. LCPU Freq. (MHz)", Some("0".into())),
        (
            "",
            "Avg. LCPU Usage (%)",
            Some(format!("{:.1}%", avg_cpu(samples))),
        ),
        ("", "Avg. GPU Freq. (MHz)", Some("0".into())),
        ("", "GPU urate", Some("0.0%".into())),
        ("", "Avg. DSU Freq. (MHz)", Some("0".into())),
        ("", "Avg. DRAM Freq. (MHz)", Some("0".into())),
        (
            "",
            "Linux PMU CPI",
            Some(format!("{:.2}", avg_cpi(samples))),
        ),
        (
            "",
            "Linux PMU Cache Miss Rate",
            Some(format!("{:.2}%", avg_cache_miss_rate(samples))),
        ),
        (
            "",
            "Linux PMU Branch Miss Rate",
            Some(format!("{:.2}%", avg_branch_miss_rate(samples))),
        ),
        ("Temperature", "Board Temp Avg (C)", Some("0.0".into())),
        ("", "Battery Temp Avg (C)", Some("0.0".into())),
        ("", "Start Temp (C)", Some("0.0".into())),
        ("", "End Temp (C)", Some("0.0".into())),
    ];

    for (index, (category, key, value)) in rows.iter().enumerate() {
        let row = (index + 2) as u32;
        if !category.is_empty() {
            set_text(ws, row, 1, category);
        }
        set_text(ws, row, 2, key);
        if let Some(value) = value {
            set_text(ws, row, 3, value);
        }
    }
    Ok(())
}

fn write_summary_vertical_sheet(
    book: &mut umya_spreadsheet::Spreadsheet,
    samples: &[LinuxHostSample],
) -> anyhow::Result<()> {
    add_sheet(book, "summary_vertical")?;
    let ws = book
        .get_sheet_by_name_mut("summary_vertical")
        .ok_or_else(|| anyhow::anyhow!("missing summary_vertical sheet"))?;
    set_text(ws, 1, 1, "Metric");
    set_text(ws, 1, 2, "Value");
    set_text(ws, 2, 1, "Samples");
    set_number(ws, 2, 2, samples.len() as f64);
    set_text(ws, 3, 1, "CPU Avg (%)");
    set_number(ws, 3, 2, avg_cpu(samples));
    set_text(ws, 4, 1, "Memory Available Min (KB)");
    set_number(ws, 4, 2, min_mem_available(samples) as f64);
    set_text(ws, 5, 1, "MIPS Avg");
    set_number(ws, 5, 2, avg_mips(samples));
    set_text(ws, 6, 1, "CPI Avg");
    set_number(ws, 6, 2, avg_cpi(samples));
    Ok(())
}

fn write_gpt_summary_sheet(
    book: &mut umya_spreadsheet::Spreadsheet,
    samples: &[LinuxHostSample],
) -> anyhow::Result<()> {
    add_sheet(book, "gpt_summary")?;
    let ws = book
        .get_sheet_by_name_mut("gpt_summary")
        .ok_or_else(|| anyhow::anyhow!("missing gpt_summary sheet"))?;
    set_text(ws, 1, 1, "Linux host local recording");
    set_text(ws, 2, 1, "Samples");
    set_number(ws, 2, 2, samples.len() as f64);
    set_text(ws, 3, 1, "Available metrics");
    set_text(
        ws,
        3,
        2,
        "CPU usage, memory total/available, process RSS, Linux PMU MIPS/MCPS/CPI/cache/branch",
    );
    Ok(())
}

fn write_power_summary_sheet(
    book: &mut umya_spreadsheet::Spreadsheet,
    samples: &[LinuxHostSample],
) -> anyhow::Result<()> {
    add_sheet(book, "Power Summary")?;
    let ws = book
        .get_sheet_by_name_mut("Power Summary")
        .ok_or_else(|| anyhow::anyhow!("missing Power Summary sheet"))?;
    set_text(ws, 1, 1, "Metric");
    set_text(ws, 1, 2, "Value");
    set_text(ws, 2, 1, "Power source");
    set_text(ws, 2, 2, "Unavailable on Linux host local mode");
    set_text(ws, 3, 1, "CPU Avg (%)");
    set_number(ws, 3, 2, avg_cpu(samples));
    set_text(ws, 4, 1, "MIPS Avg");
    set_number(ws, 4, 2, avg_mips(samples));
    Ok(())
}

fn write_raw_data_sheet(
    book: &mut umya_spreadsheet::Spreadsheet,
    samples: &[LinuxHostSample],
) -> anyhow::Result<()> {
    add_sheet(book, "RawData")?;
    let ws = book
        .get_sheet_by_name_mut("RawData")
        .ok_or_else(|| anyhow::anyhow!("missing RawData sheet"))?;
    let headers = [
        "timestamp_ms",
        "elapsed_s",
        "fps_dequeue",
        "fps_queue",
        "fps_present_fence",
        "cpu0_loading_pct",
        "cpu1_loading_pct",
        "cpu2_loading_pct",
        "cpu3_loading_pct",
        "cpu4_loading_pct",
        "cpu5_loading_pct",
        "cpu6_loading_pct",
        "cpu7_loading_pct",
        "cpu0_freq_mhz",
        "cpu1_freq_mhz",
        "cpu2_freq_mhz",
        "cpu3_freq_mhz",
        "cpu4_freq_mhz",
        "cpu5_freq_mhz",
        "cpu6_freq_mhz",
        "cpu7_freq_mhz",
        "power_mw",
        "power_avg_mw",
        "power_ma",
        "voltage_v",
        "board_temp_c",
        "battery_temp_c",
        "gpu_freq_mhz",
        "gpu_loading_pct",
        "total_mips",
        "game_mips",
        "logical_mips",
        "render_mips",
        "rhi_mips",
        "dsu_freq_mhz",
        "dram_freq_mhz",
        "vcore_v",
        "wss_kb",
        "pss_kb",
        "cpu_time_ms",
        "gpu_time_ms",
        "process_name",
        "mem_total_kb",
        "mem_available_kb",
        "top_app_rss_kb",
        "total_mips",
        "total_mcps",
        "cpi",
        "cache_references_per_sec",
        "cache_misses_per_sec",
        "cache_miss_rate_pct",
        "branch_instructions_per_sec",
        "branch_misses_per_sec",
        "branch_miss_rate_pct",
    ];
    write_row_strings(ws, 1, &headers);

    let start = samples.first().map(|s| s.timestamp_ms).unwrap_or(0);
    for (index, sample) in samples.iter().enumerate() {
        let row = (index + 2) as u32;
        set_number(ws, row, 1, sample.timestamp_ms as f64);
        set_number(
            ws,
            row,
            2,
            sample.timestamp_ms.saturating_sub(start) as f64 / 1000.0,
        );
        for col in 3..=5 {
            set_number(ws, row, col, 0.0);
        }
        for col in 6..=13 {
            set_number(ws, row, col, sample.cpu_usage_pct);
        }
        for col in 14..=41 {
            set_number(ws, row, col, 0.0);
        }
        set_text(ws, row, 42, &sample.process_name);
        set_number(ws, row, 43, sample.mem_total_kb as f64);
        set_number(ws, row, 44, sample.mem_available_kb as f64);
        set_number(ws, row, 45, sample.top_app_rss_kb as f64);
        set_number(ws, row, 46, sample.total_mips);
        set_number(ws, row, 47, sample.total_mcps);
        set_number(ws, row, 48, sample.cpi);
        set_number(ws, row, 49, sample.cache_references_per_sec);
        set_number(ws, row, 50, sample.cache_misses_per_sec);
        set_number(ws, row, 51, sample.cache_miss_rate_pct);
        set_number(ws, row, 52, sample.branch_instructions_per_sec);
        set_number(ws, row, 53, sample.branch_misses_per_sec);
        set_number(ws, row, 54, sample.branch_miss_rate_pct);
    }
    Ok(())
}

fn write_row_strings(ws: &mut umya_spreadsheet::Worksheet, row: u32, values: &[&str]) {
    for (index, value) in values.iter().enumerate() {
        set_text(ws, row, (index + 1) as u32, value);
    }
}

fn write_row_numbers(ws: &mut umya_spreadsheet::Worksheet, row: u32, values: &[f64]) {
    for (index, value) in values.iter().enumerate() {
        set_number(ws, row, (index + 1) as u32, *value);
    }
}

fn set_text(ws: &mut umya_spreadsheet::Worksheet, row: u32, col: u32, value: &str) {
    ws.get_cell_mut((col, row)).set_value(value);
}

fn add_sheet(book: &mut umya_spreadsheet::Spreadsheet, name: &str) -> anyhow::Result<()> {
    book.new_sheet(name)
        .map(|_| ())
        .map_err(|err| anyhow::anyhow!(err))
}

fn set_number(ws: &mut umya_spreadsheet::Worksheet, row: u32, col: u32, value: f64) {
    ws.get_cell_mut((col, row)).set_value_number(value);
}

fn avg_cpu(samples: &[LinuxHostSample]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().map(|s| s.cpu_usage_pct).sum::<f64>() / samples.len() as f64
}

fn avg_mips(samples: &[LinuxHostSample]) -> f64 {
    average_nonzero(samples.iter().map(|s| s.total_mips))
}

fn avg_cpi(samples: &[LinuxHostSample]) -> f64 {
    average_nonzero(samples.iter().map(|s| s.cpi))
}

fn avg_cache_miss_rate(samples: &[LinuxHostSample]) -> f64 {
    average_nonzero(samples.iter().map(|s| s.cache_miss_rate_pct))
}

fn avg_branch_miss_rate(samples: &[LinuxHostSample]) -> f64 {
    average_nonzero(samples.iter().map(|s| s.branch_miss_rate_pct))
}

fn average_nonzero(values: impl Iterator<Item = f64>) -> f64 {
    let values = values.filter(|value| *value > 0.0).collect::<Vec<_>>();
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn min_mem_available(samples: &[LinuxHostSample]) -> u64 {
    samples
        .iter()
        .map(|s| s.mem_available_kb)
        .min()
        .unwrap_or(0)
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                current.push('"');
                let _ = chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                fields.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    fields.push(current);
    fields
}

fn parse_optional_f64(fields: &[String], index: usize) -> anyhow::Result<f64> {
    match fields.get(index) {
        Some(value) if !value.trim().is_empty() => Ok(value.parse()?),
        _ => Ok(0.0),
    }
}
