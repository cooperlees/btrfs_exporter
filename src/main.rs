use clap::Parser;
use log::{debug, error, info};
use signal_hook::{consts::SIGINT, iterator::Signals};
use std::collections::HashMap;
use std::process;
use std::thread;

use anyhow::Result;
// TODO: See if we can get rid of the self here + learn what it's for
use prometheus_exporter::{self, prometheus::register_gauge_vec, prometheus::GaugeVec};
use subprocess::{Popen, PopenConfig, Redirection};

#[derive(Debug, Parser)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    mountpoints: String,
    #[clap(short, long, value_parser, default_value_t = 9899)]
    port: u32,
    #[clap(flatten)]
    verbose: clap_verbosity_flag::Verbosity,
}

// TODO - Change hashmaps to use this + implement traits to learn
#[allow(dead_code)]
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
        let dev_stats: Vec<&str> = line.split("]").collect();
        let stat_values: Vec<&str> = dev_stats[1].split_whitespace().collect();
        let dev_path: Vec<&str> = dev_stats[0].split("/").collect();
        let hash_key = format!("{}_{}", &dev_path[2].to_string(), &stat_values[0][1..]);
        device_stats.insert(hash_key, stat_values[1].parse::<f64>().unwrap());
    }
    device_stats
}

fn get_btrfs_stats(mountpoints: String) -> Result<HashMap<String, f64>> {
    let btrfs_bin = "/usr/bin/btrfs";
    let sudo_bin = "/usr/bin/sudo";
    let mut stats = HashMap::new();

    // Call btrfs CLI to get error counters
    // TODO: Learn how to thread and do a mountpoint at a time
    for mountpoint in mountpoints.split(",") {
        let cmd = Vec::from([sudo_bin, btrfs_bin, "device", "stats", &mountpoint]);
        debug!("--> Running {:?}", cmd);
        let mut p = Popen::create(
            &cmd,
            PopenConfig {
                stdout: Redirection::Pipe,
                ..Default::default()
            },
        )?;
        let (out, err) = p.communicate(None)?;
        // TODO: Workout how to get return value into error logging
        if let Some(_exit_status) = p.poll() {
            let btrfs_stats = parse_btrfs_stats(out.unwrap());
            stats.extend(btrfs_stats);
        } else {
            p.terminate()?;
            error!("{:?} failed: {:?}", cmd, err);
        }
    }

    Ok(stats)
}

fn main() -> () {
    let mut signals = Signals::new(&[SIGINT]).unwrap();
    let args = Cli::parse();
    env_logger::Builder::new()
        .filter_level(args.verbose.log_level_filter())
        .init();

    info!("Starting btrfs prometheus exporter on port {}", args.port);

    let bind_uri = format!("[::]:{}", args.port);
    let binding = bind_uri.parse().unwrap();
    let exporter = prometheus_exporter::start(binding).unwrap();

    // Add signal handler for clean exit
    thread::spawn(move || {
        for sig in signals.forever() {
            // TODO: Print signal name somehow ...
            info!("Received signal {:?}", sig);
            if sig == SIGINT {
                process::exit(0);
            }
        }
    });

    // TODO: make more accurate help to explain what they mean
    let labels = vec!["device"];
    let corruption_errs =
        register_gauge_vec!("btrfs_corruption_errs", "BTRFS Corruption Errors", &labels).unwrap();
    let flush_io_errs =
        register_gauge_vec!("btrfs_flush_io_errs", "BTRFS Flush IO Errors", &labels).unwrap();
    let generation_errs =
        register_gauge_vec!("btrfs_generation_errs", "BTRFS Generation Errors", &labels).unwrap();
    let read_io_errs =
        register_gauge_vec!("btrfs_read_io_errs", "BTRFS Read IO Errors", &labels).unwrap();
    let write_io_errs =
        register_gauge_vec!("btrfs_write_io_errs", "BTRFS Write IO Errors", &labels,).unwrap();

    loop {
        let guard = exporter.wait_request();
        let stats_hash = get_btrfs_stats(args.mountpoints.clone()).unwrap();
        debug!("Stats collected: {:?}", stats_hash);

        // TODO: Move to function passing all guages etc.
        for (k, err_count) in &stats_hash {
            let k_parts: Vec<&str> = k.split("_").collect();
            let device: String = k_parts[0].clone().to_string();
            let replace_pattern = format!("{}_", device);
            let stat_name = k.replace(&replace_pattern, "");

            let mut stat_guage: Option<&GaugeVec> = None;
            match stat_name.as_str() {
                "corruption_errs" => stat_guage = Some(&corruption_errs),
                "flush_io_errs" => stat_guage = Some(&flush_io_errs),
                "generation_errs" => stat_guage = Some(&generation_errs),
                "read_io_errs" => stat_guage = Some(&read_io_errs),
                "write_io_errs" => stat_guage = Some(&write_io_errs),
                _ => error!("{} stat not handled", stat_name),
            };
            if !stat_guage.is_none() {
                stat_guage
                    .unwrap()
                    .with_label_values(&[device.as_str()])
                    .set(*err_count);
            }
        }

        drop(guard);
        info!("{} btrfs stats collected and served", stats_hash.len());
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
