use clap::Parser;
use log::{debug, info};
use std::collections::HashMap;

use anyhow::Result;
use prometheus_exporter::{self, prometheus::register_counter};

#[derive(Debug, Parser)]
struct Cli {
    mountpoints: String,
    #[clap(short, long, value_parser, default_value_t = 9899)]
    port: u32,
    #[clap(flatten)]
    verbose: clap_verbosity_flag::Verbosity,
}

// TODO - Change hashmaps to use this + implement traits to learn
struct BtrfsErrors {
    corruption_io_errs: f64,
    flush_io_errs: f64,
    generation_io_errs: f64,
    read_io_errs: f64,
    write_io_errs: f64,
}

fn parse_btrfs_stats(stats_output: String) -> HashMap<String, f64> {
    let mut device_stats = HashMap::new();
    for line in stats_output.lines() {
        // [/dev/sdb].write_io_errs    0
        let dev_stats: Vec<&str> = line.split("]").collect();
        let stat_values: Vec<&str> = dev_stats[1].split_whitespace().collect();
        let dev_path: Vec<&str> = dev_stats[0].split("/").collect();
        let hash_key = format!("{}_{}", &dev_path[2].to_string(), &stat_values[0][1..]);
        device_stats.insert(hash_key, stat_values[1].parse::<f64>().unwrap());
    }
    device_stats
}

fn get_btrfs_stats(mountpoints: Vec<String>) -> Result<HashMap<String, f64>> {
    let btrfs_bin = "/usr/bin/btrfs";
    let sudo_bin = "/usr/bin/sudo";
    let mut stats = HashMap::new();

    // Call x to get error counters
    for mountpoint in mountpoints {
        let cmd = Vec::from([sudo_bin, btrfs_bin, "device", "stats", &mountpoint]);
        debug!("--> Running {:?}", cmd);
        // let btrfs_errors = Exec::cmd("sort").capture()?.stdout_str();
    }

    Ok(stats)
}

fn main() -> () {
    let args = Cli::parse();
    env_logger::Builder::new()
        .filter_level(args.verbose.log_level_filter())
        .init();

    info!("Starting btrfs prometheus exporter on port {}", args.port);

    let bind_uri = format!("0.0.0.0:{}", args.port);
    let binding = bind_uri.parse().unwrap();
    let exporter = prometheus_exporter::start(binding).unwrap();

    let counter = register_counter!("example_exporter_counter", "help").unwrap();
    loop {
        let guard = exporter.wait_request();
        counter.inc();
        drop(guard);
    }
}

#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn test_parsing_btrfs_errs() {
        let btrfs_error_output = "[/dev/sdb].write_io_errs    0
[/dev/sdb].read_io_errs     0
[/dev/sdc].write_io_errs    69";
        let mut expected_stats_map: HashMap<String, f64> = HashMap::new();
        expected_stats_map.insert("sdb_write_io_errs".to_string(), 0.0);
        expected_stats_map.insert("sdb_read_io_errs".to_string(), 0.0);
        expected_stats_map.insert("sdc_write_io_errs".to_string(), 69.0);
        assert_eq!(
            expected_stats_map,
            parse_btrfs_stats(btrfs_error_output.to_string())
        );
    }
}
