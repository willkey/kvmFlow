use std::collections::BTreeSet;

use kvmflow_diagnostics::{ProcessRecord, summarize_process_tree};

fn record(
    pid: u32,
    parent_pid: Option<u32>,
    command: &str,
    resident_bytes: u64,
    cpu_usage: f32,
) -> ProcessRecord {
    ProcessRecord {
        pid,
        parent_pid,
        command: command.to_owned(),
        resident_bytes,
        cpu_usage,
    }
}

#[test]
fn includes_root_descendants_and_bundle_helpers_only() {
    let records = [
        record(10, None, "controller", 100, 10.0),
        record(11, Some(10), "child", 200, 20.0),
        record(12, Some(11), "grandchild", 300, 30.0),
        record(20, None, "unrelated", 400, 40.0),
        record(
            30,
            None,
            "helper --bundle com.kvmflow.controller",
            500,
            50.0,
        ),
    ];

    let summary = summarize_process_tree(&records, 10, "com.kvmflow.controller", 8);

    assert_eq!(summary.selected_pids, BTreeSet::from([10_u32, 11, 12, 30]));
}

#[test]
fn sums_memory_and_process_cpu_as_an_entire_machine_percentage() {
    let records = [
        record(10, None, "controller", 100, 75.0),
        record(11, Some(10), "child", 200, 50.0),
        record(12, Some(11), "grandchild", 300, 25.0),
        record(20, None, "unrelated", 400, 400.0),
        record(
            30,
            None,
            "helper --bundle com.kvmflow.controller",
            500,
            10.0,
        ),
    ];

    let summary = summarize_process_tree(&records, 10, "com.kvmflow.controller", 4);

    assert_eq!(summary.resident_bytes, 1_100);
    assert_eq!(summary.process_count, 4);
    assert_eq!(summary.cpu_percent, 40.0);
}
