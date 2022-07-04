# btrfs_exporter

Export useful btrfs filesystem counters to prometheus.

## Run example

```shell
cooper@home1:~/repos/btrfs_exporter$ ./target/debug/btrfs_exporter -vvvvv /cesspool,/data
[2022-07-04T20:07:07Z INFO  btrfs_exporter] Starting btrfs prometheus exporter on port 9899
[2022-07-04T20:07:07Z DEBUG tiny_http] Server listening on 0.0.0.0:9899
[2022-07-04T20:07:07Z INFO  prometheus_exporter] exporting metrics to http://0.0.0.0:9899/metrics
[2022-07-04T20:07:07Z DEBUG tiny_http] Running accept thread
[2022-07-04T20:07:10Z DEBUG btrfs_exporter] --> Running ["/usr/bin/sudo", "/usr/bin/btrfs", "device", "stats", "/cesspool"]
[2022-07-04T20:07:10Z DEBUG btrfs_exporter] --> Running ["/usr/bin/sudo", "/usr/bin/btrfs", "device", "stats", "/data"]
[2022-07-04T20:07:10Z DEBUG btrfs_exporter] Stats collected: {"sdb_write_io_errs": 0.0, "sdc_read_io_errs": 0.0, "sdd_generation_errs": 0.0, "sdd_read_io_errs": 0.0, "sdc_write_io_errs": 0.0, "sdc_generation_errs": 0.0, "sdd_write_io_errs": 0.0, "sdb_flush_io_errs": 0.0, "sdb_generation_errs": 0.0, "sdd_flush_io_errs": 0.0, "sdb_corruption_errs": 0.0, "sdc_corruption_errs": 0.0, "sdb_read_io_errs": 0.0, "sdd_corruption_errs": 0.0, "sdc_flush_io_errs": 0.0}
[2022-07-04T20:07:10Z INFO  btrfs_exporter] 15 btrfs stats collected and served
```

## Example Stats exported per device
```shell
# HELP btrfs_generation_errs BTRFS Generation Errors
# TYPE btrfs_generation_errs gauge
btrfs_generation_errs{device="sdb"} 0
btrfs_generation_errs{device="sdc"} 0
btrfs_generation_errs{device="sdd"} 0
# HELP btrfs_read_io_errs BTRFS Read IO Errors
# TYPE btrfs_read_io_errs gauge
btrfs_read_io_errs{device="sdb"} 0
btrfs_read_io_errs{device="sdc"} 0
btrfs_read_io_errs{device="sdd"} 0
# HELP btrfs_write_io_errs BTRFS Write IO Errors
# TYPE btrfs_write_io_errs gauge
btrfs_write_io_errs{device="sdb"} 0
btrfs_write_io_errs{device="sdc"} 0
btrfs_write_io_errs{device="sdd"} 0
```
