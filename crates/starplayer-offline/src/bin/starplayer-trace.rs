//! Implementation detail behind `cargo xtask trace`.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use starplayer_offline::{TraceOptions, trace_s3m};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("trace: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let module_path = arguments.next().ok_or("usage: cargo xtask trace <module> [--ticks N]")?;
    let mut ticks = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--ticks" => {
                let value = arguments.next().ok_or("`--ticks` needs a value")?;
                ticks = Some(value.parse::<usize>().map_err(|_| format!("invalid tick count `{value}`"))?);
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }

    let bytes = std::fs::read(&module_path).map_err(|error| format!("could not read `{module_path}`: {error}"))?;
    let trace = trace_s3m(&bytes, TraceOptions { ticks, ..TraceOptions::default() }).map_err(|error| error.to_string())?;
    print!("{}", trace.to_text());
    Ok(())
}
