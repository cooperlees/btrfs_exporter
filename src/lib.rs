use std::io::stderr;
use std::io::IsTerminal;

use clap::ValueEnum;
use tracing_glog::Glog;
use tracing_glog::GlogFields;
use tracing_opentelemetry::layer as otel_layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::Registry;

// This enum can be used to add `log-level` option to CLI binaries.
#[derive(ValueEnum, Clone, Debug, Copy)]
pub enum LogLevels {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<LogLevels> for LevelFilter {
    fn from(public_level: LogLevels) -> Self {
        match public_level {
            LogLevels::Error => LevelFilter::ERROR,
            LogLevels::Warn => LevelFilter::WARN,
            LogLevels::Info => LevelFilter::INFO,
            LogLevels::Debug => LevelFilter::DEBUG,
            LogLevels::Trace => LevelFilter::TRACE,
        }
    }
}

pub fn setup_logging<T>(log_filter_level: LevelFilter, otel_tracer: Option<T>)
where
    T: opentelemetry::trace::Tracer + Send + Sync + 'static,
    T::Span: Send + Sync + 'static,
{
    let fmt = fmt::Layer::default()
        .with_writer(std::io::stderr)
        .with_ansi(stderr().is_terminal())
        .event_format(Glog::default().with_timer(tracing_glog::LocalTime::default()))
        .fmt_fields(GlogFields::default())
        .with_filter(log_filter_level);

    let registry = Registry::default().with(fmt);
    if let Some(tracer) = otel_tracer {
        let subscriber = registry.with(otel_layer().with_tracer(tracer));
        tracing::subscriber::set_global_default(subscriber)
            .expect("Unable to set global tracing subscriber");
    } else {
        tracing::subscriber::set_global_default(registry)
            .expect("Unable to set global tracing subscriber");
    }
}
