//! `caravel`: the deploy tool's front door. `caravel <command> <lane file> …`
//! reads the lane file's `[app] template` and runs the same command with that
//! template's node binary, `caravel-<template>-node`, which carries the app's
//! genesis. Installed as `stellar-caravel`, it is also a Stellar CLI plugin:
//! `stellar caravel plan …`.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const USAGE: &str = "usage: caravel <plan|apply> <lane file> --env <name> [options]

  plan    show what apply would change on Stellar and on the host
  apply   make them match the lane file's [env.<name>] deployment

Run `caravel-<template>-node <command> --help` for a command's options.";

/// The template's node binary: next to this one (a `stellar-caravel` link
/// resolved), else on PATH.
fn node_binary(template: &str) -> PathBuf {
    let name = format!("caravel-{template}-node");
    std::env::current_exe()
        .ok()
        .and_then(|me| std::fs::canonicalize(me).ok())
        .and_then(|me| me.parent().map(|d| d.join(&name)))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn template(lane: &Path) -> Result<String, String> {
    let text =
        std::fs::read_to_string(lane).map_err(|e| format!("reading {}: {e}", lane.display()))?;
    let t: toml::Table = toml::from_str(&text).map_err(|e| format!("{}: {e}", lane.display()))?;
    t.get("app")
        .and_then(|a| a.get("template"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("{} has no [app] template", lane.display()))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cmd), Some(lane)) = (args.first(), args.get(1)) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    if cmd.starts_with('-') || lane.starts_with('-') {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    let t = template(Path::new(lane)).unwrap_or_else(|e| {
        eprintln!("caravel: {e}");
        std::process::exit(2);
    });
    let bin = node_binary(&t);
    let err = Command::new(&bin).args(&args).exec();
    eprintln!("caravel: running {}: {err}", bin.display());
    std::process::exit(1);
}
