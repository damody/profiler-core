use profiler_core::local_linux_record::{
    generate_linux_host_gar_xlsx, parse_local_linux_csv, rtb_csv_header, rtb_csv_row,
    LinuxThreadSummary,
};
use profiler_core::proto::RtbDataPoint;

#[test]
fn formats_linux_host_rtb_samples_as_csv() {
    let sample = RtbDataPoint {
        timestamp_ms: 1234,
        process_name: "demo".to_string(),
        cpu_usages_pct: vec![12.5],
        bcpu_usage_pct: 12.5,
        mem_total_kb: 1024,
        mem_available_kb: 768,
        top_app_rss_kb: 256,
        total_mips: 10.0,
        total_mcps: 20.0,
        cpi: 2.0,
        cache_references_per_sec: 1000.0,
        cache_misses_per_sec: 100.0,
        cache_miss_rate_pct: 10.0,
        branch_instructions_per_sec: 500.0,
        branch_misses_per_sec: 25.0,
        branch_miss_rate_pct: 5.0,
        ..RtbDataPoint::default()
    };

    assert_eq!(
        rtb_csv_header(),
        "timestamp_ms,process_name,cpu_usage_pct,bcpu_usage_pct,mem_total_kb,mem_available_kb,top_app_rss_kb,total_mips,total_mcps,cpi,cache_references_per_sec,cache_misses_per_sec,cache_miss_rate_pct,branch_instructions_per_sec,branch_misses_per_sec,branch_miss_rate_pct"
    );
    assert_eq!(
        rtb_csv_row(&sample),
        "1234,demo,12.500,12.500,1024,768,256,10.000000,20.000000,2.000000,1000.000,100.000,10.000000,500.000,25.000,5.000000"
    );
}

#[test]
fn generates_csharp_compatible_linux_host_gar_workbook() {
    let dir = std::env::temp_dir().join(format!(
        "mprofiler_gar_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let csv_path = dir.join("record.csv");
    let xlsx_path = dir.join("GAR.xlsx");
    std::fs::write(
        &csv_path,
        concat!(
            "timestamp_ms,process_name,cpu_usage_pct,bcpu_usage_pct,mem_total_kb,mem_available_kb,top_app_rss_kb\n",
            "1000,linux-host-local,0.000,0.000,1000,800,0\n",
            "1500,linux-host-local,25.500,25.500,1000,700,128\n",
        ),
    )
    .unwrap();

    let samples = parse_local_linux_csv(&csv_path).unwrap();
    let threads = vec![LinuxThreadSummary {
        pid: 111,
        tgid: 111,
        name: "heavy-thread".to_string(),
        loading_pct: 76.5,
        c0_pct: 0.0,
        c1_pct: 0.0,
        c2_pct: 76.5,
        runnable_pct: 76.5,
        mips: 1234.0,
        mcps: 2345.0,
        cpi: 1.9,
    }];
    generate_linux_host_gar_xlsx(&xlsx_path, &samples, &threads).unwrap();

    let book = umya_spreadsheet::reader::xlsx::read(&xlsx_path).unwrap();
    assert!(book.get_sheet_by_name("summary").is_some());
    assert!(book.get_sheet_by_name("Advanced Report").is_some());
    assert!(book.get_sheet_by_name("RawData").is_some());
    assert!(book.get_sheet_by_name("Power Summary").is_some());
    assert!(book.get_sheet_by_name("gpt_summary").is_some());

    let advanced = book.get_sheet_by_name("Advanced Report").unwrap();
    assert_eq!(advanced.get_value("A1"), "System Index*");
    assert_eq!(advanced.get_value("B1"), "Key Index");
    assert_eq!(advanced.get_value("C1"), "Linux Host Local");
    assert_eq!(advanced.get_value("B2"), "Avg FPS");
    assert_eq!(advanced.get_value("B19"), "Avg. BCPU Freq. (MHz)");

    let raw = book.get_sheet_by_name("RawData").unwrap();
    assert_eq!(raw.get_value("A1"), "timestamp_ms");
    assert_eq!(raw.get_value("B1"), "elapsed_s");
    assert_eq!(raw.get_value("F1"), "cpu0_loading_pct");
    assert_eq!(raw.get_value("AK1"), "vcore_v");
    assert_eq!(raw.get_value("A3"), "1500");
    assert_eq!(raw.get_value("F3"), "25.5");

    let summary = book.get_sheet_by_name("summary").unwrap();
    assert_eq!(summary.get_value("A2"), "FPS Avg");
    assert_eq!(summary.get_value("A5"), "CPU C0 Freq");
    assert_eq!(summary.get_value("C6"), "12.75");
    assert_eq!(summary.get_value("A23"), "111");
    assert_eq!(summary.get_value("C23"), "heavy-thread");
    assert_eq!(summary.get_value("D23"), "76.5");
    assert_eq!(summary.get_value("I23"), "1234");

    std::fs::remove_dir_all(dir).unwrap();
}
