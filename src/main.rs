use std::collections::HashMap;
use std::process;
use std::thread;

use clap::Parser;
use signal_hook::{consts::SIGINT, iterator::Signals};
use tracing::{debug, error, info};

use anyhow::Result;
// TODO: See if we can get rid of the self here + learn what it's for
use prometheus_exporter::{self, prometheus::register_gauge_vec, prometheus::GaugeVec};
use subprocess::{Popen, PopenConfig, Redirection};


#[derive(Debug, Parser)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// Mountpoints to grab stats for
    mountpoints: String,
    /// Port to listen on
    #[clap(short, long, value_parser, default_value_t = 9899)]
    port: u32,
    /// Adjust the console log-level
    #[arg(long, short, value_enum, ignore_case = true, default_value = "Info")]
    log_level: btrfs_exporter::LogLevels,
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
        let dev_stats: Vec<&str> = line.split(']').collect();
        let stat_values: Vec<&str> = dev_stats[1].split_whitespace().collect();
        let dev_path: Vec<&str> = dev_stats[0].split('/').collect();
        let hash_key = format!("{}_{}", &dev_path[2].to_string(), &stat_values[0][1..]);
        device_stats.insert(hash_key, stat_values[1].parse::<f64>().unwrap());
    }
    device_stats
}

fn _fork_btrfs(cmd: Vec<String>) -> Result<HashMap<String, f64>> {
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
        return Ok(parse_btrfs_stats(out.unwrap()));
    } else {
        p.terminate()?;
        error!("{:?} failed: {:?}", cmd, err);
    }
    Ok(HashMap::new())
}

fn get_btrfs_stats(mountpoints: String) -> Result<HashMap<String, f64>> {
    let btrfs_bin = "/usr/bin/btrfs".to_string();
    let sudo_bin = "/usr/bin/sudo".to_string();

    // Call btrfs CLI to get error counters
    let mut btrfs_threads: Vec<thread::JoinHandle<Result<HashMap<String, f64>>>> = vec![];
    for mountpoint in mountpoints.split(',') {
        let cmd = Vec::from([
            sudo_bin.clone(),
            btrfs_bin.clone(),
            "device".to_string(),
            "stats".to_string(),
            mountpoint.to_string(),
        ]);
        debug!("--> Making a thread to run {:?}", cmd);
        btrfs_threads.push(thread::spawn(|| _fork_btrfs(cmd)))
    }

    // Collect the stats from each thread
    let mut stats: HashMap<String, f64> = HashMap::new();
    for thread in btrfs_threads.into_iter() {
        match thread.join().unwrap() {
            Ok(stat_hash) => stats.extend(stat_hash),
            Err(_) => continue, // error is logged in function ...
        }
    }

    Ok(stats)
}

fn main() {
    let mut signals = Signals::new([SIGINT]).unwrap();
    let args = Cli::parse();
    btrfs_exporter::setup_logging(args.log_level.into());

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

    // https://btrfs.readthedocs.io/en/latest/btrfs-device.html#device-stats
    let labels = vec!["device"];
    let corruption_errs = register_gauge_vec!(
        "btrfs_corruption_errs",
        "A block checksum mismatched or a corrupted metadata header was found.",
        &labels
    )
    .unwrap();
    let flush_io_errs =
        register_gauge_vec!(
            "btrfs_flush_io_errs",
            concat!(
                "Number of failed writes with the FLUSH flag set. The flushing is a method of forcing a particular order between write ",
                "requests and is crucial for implementing crash consistency. In case of btrfs, all the metadata blocks must be permanently ",
                "stored on the block device before the superblock is written.",
            ),
            &labels
        ).unwrap();
    let generation_errs = register_gauge_vec!(
        "btrfs_generation_errs",
        "The block generation does not match the expected value (eg. stored in the parent node).",
        &labels
    )
    .unwrap();
    let read_io_errs =
        register_gauge_vec!(
            "btrfs_read_io_errs",
            "Failed reads to the block devices, means that the layers beneath the filesystem were not able to satisfy the read request.",
            &labels
        ).unwrap();
    let write_io_errs =
        register_gauge_vec!(
            "btrfs_write_io_errs",
            "Failed writes to the block devices, means that the layers beneath the filesystem were not able to satisfy the write request.",
            &labels,
        ).unwrap();

    loop {
        let guard = exporter.wait_request();
        let stats_hash = get_btrfs_stats(args.mountpoints.clone()).unwrap();
        debug!("Stats collected: {:?}", stats_hash);

        // TODO: Move to function passing all guages etc.
        for (k, err_count) in &stats_hash {
            let k_parts: Vec<&str> = k.split('_').collect();
            let device: String = k_parts[0].to_string();
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
            if let Some(stat_guage_value) = stat_guage {
                stat_guage_value
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
