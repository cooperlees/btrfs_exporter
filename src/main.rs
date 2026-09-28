use std::collections::HashMap;
use std::process;

use clap::Parser;
use futures::future::join_all;
use tokio::process::Command;
use tokio::signal;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info};

use anyhow::Result;
#[cfg(feature = "otel")]
use opentelemetry::trace::TracerProvider as _;
#[cfg(feature = "otel")]
use opentelemetry_otlp::WithExportConfig;
#[cfg(feature = "otel")]
use opentelemetry_sdk as otel_sdk;
// TODO: See if we can get rid of the self here + learn what it's for
use prometheus_exporter::{prometheus::register_gauge_vec, prometheus::GaugeVec};

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    /// Mountpoints to grab stats for
    mountpoints: String,
    /// Port to listen on
    #[arg(short, long, default_value_t = 9899)]
    port: u32,
    /// Adjust the console log-level
    #[arg(long, short, value_enum, ignore_case = true, default_value = "Info")]
    log_level: btrfs_exporter::LogLevels,
    /// Opentelemetry endpoint
    #[cfg(feature = "otel")]
    #[arg(long, short, default_value = "http://127.0.0.1:4317")]
    opentelemetry: String,
}

/// A single error counter for one device, e.g. `("sda", "write_io_errs") -> 3.0`.
///
/// `btrfs device stats` lines look like `[/dev/sda].write_io_errs   3`;
/// parsing yields the device name plus the stat name separately so callers
/// never have to split a joined string key back apart.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DeviceStat {
    device: String,
    stat: String,
}

/// Parse one `btrfs device stats` output line.
/// Returns `None` (and logs a warning) for malformed lines instead of
/// panicking, so one unexpected line can't take down the whole exporter.
#[tracing::instrument]
fn parse_btrfs_stats_line(line: &str) -> Option<(DeviceStat, f64)> {
    // Expected shape: "[/dev/<device>].<stat_name>   <value>"
    let (device_part, rest) = line.split_once(']')?;
    let device = device_part.rsplit('/').next().filter(|s| !s.is_empty())?;
    let rest = rest.strip_prefix('.')?;
    let (stat_name, value_str) = rest.split_once(char::is_whitespace)?;
    if stat_name.is_empty() {
        return None;
    }
    let value: f64 = value_str.trim().parse().ok()?;
    Some((
        DeviceStat {
            device: device.to_string(),
            stat: stat_name.to_string(),
        },
        value,
    ))
}

#[tracing::instrument]
fn parse_btrfs_stats(stats_output: &str) -> HashMap<DeviceStat, f64> {
    let mut device_stats = HashMap::new();
    for line in stats_output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match parse_btrfs_stats_line(line) {
            Some((key, value)) => {
                device_stats.insert(key, value);
            }
            None => error!("Skipping malformed btrfs stats line: {:?}", line),
        }
    }
    device_stats
}

#[tracing::instrument]
async fn fork_btrfs(cmd: Vec<String>) -> Result<HashMap<DeviceStat, f64>> {
    let command_timeout = Duration::from_secs(30);
    let output = match timeout(
        command_timeout,
        Command::new(&cmd[0]).args(&cmd[1..]).output(),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            error!("{:?} timed out after {:?}", cmd, command_timeout);
            return Ok(HashMap::new());
        }
    };

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Ok(parse_btrfs_stats(&stdout));
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("{:?} failed: {:?}", cmd, stderr);
    }
    Ok(HashMap::new())
}

#[tracing::instrument]
async fn get_btrfs_stats(mountpoints: String) -> Result<HashMap<DeviceStat, f64>> {
    let btrfs_bin = "/usr/bin/btrfs".to_string();
    let sudo_bin = "/usr/bin/sudo".to_string();

    // Call btrfs CLI to get error counters
    let mut tasks = vec![];
    for mountpoint in mountpoints.split(',') {
        let cmd = Vec::from([
            sudo_bin.clone(),
            btrfs_bin.clone(),
            "device".to_string(),
            "stats".to_string(),
            mountpoint.to_string(),
        ]);
        debug!("--> Spawning async task to run {:?}", cmd);
        tasks.push(tokio::spawn(fork_btrfs(cmd)));
    }

    // Collect the stats from each task
    let mut stats: HashMap<DeviceStat, f64> = HashMap::new();
    let results = join_all(tasks).await;
    for result in results {
        match result {
            Ok(Ok(stat_hash)) => stats.extend(stat_hash),
            Ok(Err(e)) => error!("Task failed: {:?}", e),
            Err(e) => error!("Join error: {:?}", e),
        }
    }

    Ok(stats)
}

#[tokio::main(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn main() -> Result<(), anyhow::Error> {
    let args = Cli::parse();

    #[cfg(feature = "otel")]
    {
        opentelemetry::global::set_text_map_propagator(
            otel_sdk::propagation::TraceContextPropagator::new(),
        );

        // Build OTLP exporter for traces
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&args.opentelemetry)
            .build()?;

        // Create tracer provider with resource attributes
        let tracer_provider = otel_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(
                otel_sdk::Resource::builder()
                    .with_service_name("btrfs_exporter")
                    .with_attribute(opentelemetry::KeyValue::new(
                        "service.version",
                        env!("CARGO_PKG_VERSION"),
                    ))
                    .build(),
            )
            .build();

        let tracer = tracer_provider.tracer("btrfs_exporter");
        opentelemetry::global::set_tracer_provider(tracer_provider.clone());

        btrfs_exporter::setup_logging(args.log_level.into(), Some(tracer));

        // Add async signal handler for clean exit with tracer flush
        let provider_for_shutdown = tracer_provider.clone();
        tokio::spawn(async move {
            match signal::ctrl_c().await {
                Ok(()) => {
                    info!("Received SIGINT, shutting down");
                    if let Err(e) = provider_for_shutdown.shutdown() {
                        error!("Failed to shut down tracer provider: {:?}", e);
                    }
                    process::exit(0);
                }
                Err(e) => {
                    error!("Failed to listen for SIGINT: {:?}", e);
                }
            }
        });
    }

    #[cfg(not(feature = "otel"))]
    {
        btrfs_exporter::setup_logging(args.log_level.into());

        // Add async signal handler for clean exit
        tokio::spawn(async move {
            match signal::ctrl_c().await {
                Ok(()) => {
                    info!("Received SIGINT, shutting down");
                    process::exit(0);
                }
                Err(e) => {
                    error!("Failed to listen for SIGINT: {:?}", e);
                }
            }
        });
    }

    info!("Starting btrfs prometheus exporter on port {}", args.port);

    let bind_uri = format!("[::]:{}", args.port);
    let binding = bind_uri.parse().expect("Failed to parse bind URI");
    let exporter =
        prometheus_exporter::start(binding).expect("Failed to start prometheus exporter");

    // https://btrfs.readthedocs.io/en/latest/btrfs-device.html#device-stats
    let labels = vec!["device"];
    let corruption_errs = register_gauge_vec!(
        "btrfs_corruption_errs",
        "A block checksum mismatched or a corrupted metadata header was found.",
        &labels
    )
    .expect("Failed to register btrfs_corruption_errs gauge");
    let flush_io_errs =
        register_gauge_vec!(
            "btrfs_flush_io_errs",
            concat!(
                "Number of failed writes with the FLUSH flag set. The flushing is a method of forcing a particular order between write ",
                "requests and is crucial for implementing crash consistency. In case of btrfs, all the metadata blocks must be permanently ",
                "stored on the block device before the superblock is written.",
            ),
            &labels
        ).expect("Failed to register btrfs_flush_io_errs gauge");
    let generation_errs = register_gauge_vec!(
        "btrfs_generation_errs",
        "The block generation does not match the expected value (eg. stored in the parent node).",
        &labels
    )
    .expect("Failed to register btrfs_generation_errs gauge");
    let read_io_errs =
        register_gauge_vec!(
            "btrfs_read_io_errs",
            "Failed reads to the block devices, means that the layers beneath the filesystem were not able to satisfy the read request.",
            &labels
        ).expect("Failed to register btrfs_read_io_errs gauge");
    let write_io_errs =
        register_gauge_vec!(
            "btrfs_write_io_errs",
            "Failed writes to the block devices, means that the layers beneath the filesystem were not able to satisfy the write request.",
            &labels,
        ).expect("Failed to register btrfs_write_io_errs gauge");

    // Note: wait_request() blocks until a request arrives, then returns a guard.
    // The guard is held across the await, but this is intentional - we need to hold the guard
    // while collecting and setting metrics, then drop it to send the response.
    loop {
        let guard = exporter.wait_request();

        // Collect stats on-demand after receiving the scrape request
        let stats_hash = get_btrfs_stats(args.mountpoints.clone())
            .await
            .expect("Failed to get btrfs stats");
        debug!("Stats collected: {:?}", stats_hash);

        // Update gauges with collected stats
        for (stat, err_count) in &stats_hash {
            let stat_gauge: Option<&GaugeVec> = match stat.stat.as_str() {
                "corruption_errs" => Some(&corruption_errs),
                "flush_io_errs" => Some(&flush_io_errs),
                "generation_errs" => Some(&generation_errs),
                "read_io_errs" => Some(&read_io_errs),
                "write_io_errs" => Some(&write_io_errs),
                _ => {
                    error!("{} stat not handled", stat.stat);
                    None
                }
            };
            if let Some(stat_gauge_value) = stat_gauge {
                stat_gauge_value
                    .with_label_values(&[stat.device.as_str()])
                    .set(*err_count);
            }
        }
        info!("{} btrfs stats collected and served", stats_hash.len());
        drop(guard);
    }
}

#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    fn device_stat(device: &str, stat: &str) -> DeviceStat {
        DeviceStat {
            device: device.to_string(),
            stat: stat.to_string(),
        }
    }

    #[test]
    fn test_parsing_btrfs_errs() {
        let btrfs_error_output = "[/dev/sdb].write_io_errs    0
[/dev/sdb].read_io_errs     0
[/dev/sdc].write_io_errs    69";
        let mut expected_stats_map: HashMap<DeviceStat, f64> = HashMap::new();
        expected_stats_map.insert(device_stat("sdb", "write_io_errs"), 0.0);
        expected_stats_map.insert(device_stat("sdb", "read_io_errs"), 0.0);
        expected_stats_map.insert(device_stat("sdc", "write_io_errs"), 69.0);
        assert_eq!(expected_stats_map, parse_btrfs_stats(btrfs_error_output));
    }

    #[test]
    fn test_malformed_lines_are_skipped_not_panics() {
        // Malformed lines must be skipped; previously every one of these
        // would have panicked on indexing or .expect().
        for bad_line in [
            "",
            "   ",
            "garbage without brackets",
            "[/dev/sda].write_io_errs",        // missing value
            "[/dev/sda] write_io_errs 3",      // missing dot separator
            "[/dev/sda].write_io_errs banana", // non-numeric value
            "[/dev/].write_io_errs 3",         // missing device name
            "[/dev/sda]. 3",                   // missing stat name
            ".write_io_errs 3",                // missing device part
        ] {
            assert_eq!(
                None,
                parse_btrfs_stats_line(bad_line),
                "line should be rejected: {:?}",
                bad_line
            );
        }
        // ... and a valid line next to garbage still parses.
        let mixed = "garbage line\n[/dev/nvme0n1].read_io_errs    42\n[/dev/].oops\n";
        let mut expected: HashMap<DeviceStat, f64> = HashMap::new();
        expected.insert(device_stat("nvme0n1", "read_io_errs"), 42.0);
        assert_eq!(expected, parse_btrfs_stats(mixed));
    }

    #[test]
    fn test_device_names_with_underscores_survive() {
        // The old "device_stat" string-key scheme split on '_' and
        // mis-attributed stats for devices containing underscores.
        let output = "[/dev/mapper/vg_data-lv0].write_io_errs    7";
        let stats = parse_btrfs_stats(output);
        assert_eq!(
            Some(&7.0),
            stats.get(&device_stat("vg_data-lv0", "write_io_errs"))
        );
    }
}
